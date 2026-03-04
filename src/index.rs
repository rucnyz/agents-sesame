use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use chrono::DateTime;
use tantivy::collector::TopDocs;
use tantivy::query::{
    AllQuery, BooleanQuery, BoostQuery, FastFieldRangeQuery, FuzzyTermQuery, Occur, QueryParser,
    RegexQuery, TermQuery, TermSetQuery,
};
use tantivy::schema::*;
use tantivy::{Index, IndexReader, IndexWriter, Order, ReloadPolicy, Searcher, Term, doc};

use crate::config;
use crate::query::{DateFilter, DateOp, Filter};
use crate::session::Session;

const SCHEMA_VERSION: u32 = 22; // Fast fields for metadata
const LOCK_FILE: &str = ".tantivy-writer.lock";
const WRITER_MEMORY: usize = 50_000_000; // 50MB
const SCAN_MARKER: &str = ".last_scan";

pub struct TantivyIndex {
    index_path: PathBuf,
    index: Option<Index>,
    reader: Option<IndexReader>,
    schema: Schema,
    // Field handles
    f_id: Field,
    f_title: Field,
    f_directory: Field,
    f_agent: Field,
    f_content: Field,
    f_timestamp: Field,
    f_message_count: Field,
    f_mtime: Field,
    f_yolo: Field,
}

impl Default for TantivyIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl TantivyIndex {
    pub fn new() -> Self {
        Self::new_with_path(config::index_dir())
    }

    pub fn new_with_path(index_path: PathBuf) -> Self {
        let mut builder = Schema::builder();
        let f_id = builder.add_text_field("id", (STRING | STORED).set_fast(None));
        let f_title = builder.add_text_field("title", (TEXT | STORED).set_fast(None));
        let f_directory = builder.add_text_field("directory", (STRING | STORED).set_fast(None));
        let f_agent = builder.add_text_field("agent", (STRING | STORED).set_fast(None));
        let f_content = builder.add_text_field("content", TEXT | STORED);
        let f_timestamp = builder.add_f64_field(
            "timestamp",
            NumericOptions::default()
                .set_indexed()
                .set_stored()
                .set_fast(),
        );
        let f_message_count = builder.add_i64_field(
            "message_count",
            NumericOptions::default()
                .set_stored()
                .set_indexed()
                .set_fast(),
        );
        let f_mtime =
            builder.add_f64_field("mtime", NumericOptions::default().set_stored().set_fast());
        let f_yolo =
            builder.add_bool_field("yolo", NumericOptions::default().set_stored().set_fast());
        let schema = builder.build();

        let mut idx = TantivyIndex {
            index_path,
            index: None,
            reader: None,
            schema,
            f_id,
            f_title,
            f_directory,
            f_agent,
            f_content,
            f_timestamp,
            f_message_count,
            f_mtime,
            f_yolo,
        };
        idx.ensure_index();
        idx
    }

    fn version_file(&self) -> PathBuf {
        self.index_path.join(".schema_version")
    }

    fn check_version(&self) -> bool {
        self.version_file()
            .exists()
            .then(|| fs::read_to_string(self.version_file()).ok())
            .flatten()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .is_some_and(|v| v == SCHEMA_VERSION)
    }

    fn write_version(&self) {
        let _ = fs::write(self.version_file(), SCHEMA_VERSION.to_string());
    }

    fn ensure_index(&mut self) {
        let _ = fs::create_dir_all(&self.index_path);

        // Check schema version
        if !self.check_version() {
            // Wipe and rebuild
            let _ = fs::remove_dir_all(&self.index_path);
            let _ = fs::create_dir_all(&self.index_path);
        }

        let index = match Index::create_in_dir(&self.index_path, self.schema.clone()) {
            Ok(idx) => idx,
            Err(_) => match Index::open_in_dir(&self.index_path) {
                Ok(idx) => idx,
                Err(_) => {
                    // Last resort: wipe and recreate
                    let _ = fs::remove_dir_all(&self.index_path);
                    let _ = fs::create_dir_all(&self.index_path);
                    Index::create_in_dir(&self.index_path, self.schema.clone())
                        .expect("failed to create tantivy index")
                }
            },
        };

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .expect("failed to create index reader");

        self.write_version();
        self.reader = Some(reader);
        self.index = Some(index);
    }

    fn reload_reader(&self) {
        if let Some(reader) = &self.reader {
            let _ = reader.reload();
        }
    }

    /// Check if the index was scanned recently (within `max_age` seconds).
    pub fn is_fresh(&self, max_age_secs: u64) -> bool {
        let marker = self.index_path.join(SCAN_MARKER);
        marker
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age.as_secs() < max_age_secs)
    }

    /// Touch the scan marker to record the current time.
    pub fn touch_scan_marker(&self) {
        let marker = self.index_path.join(SCAN_MARKER);
        // Write current timestamp to update mtime
        let _ = fs::write(&marker, "");
    }

    /// Remove the scan marker to force the next scan to re-read from adapters.
    pub fn invalidate_scan_marker(&self) {
        let marker = self.index_path.join(SCAN_MARKER);
        let _ = fs::remove_file(&marker);
    }

    fn searcher(&self) -> Searcher {
        self.reader.as_ref().unwrap().searcher()
    }

    fn acquire_writer(&self) -> Option<IndexWriter> {
        let index = self.index.as_ref()?;
        match index.writer(WRITER_MEMORY) {
            Ok(w) => Some(w),
            Err(_) => {
                // Try removing stale lock
                let lock_path = self.index_path.join(LOCK_FILE);
                if lock_path.exists() {
                    let _ = fs::remove_file(&lock_path);
                    index.writer(WRITER_MEMORY).ok()
                } else {
                    None
                }
            }
        }
    }

    fn session_to_doc(&self, session: &Session) -> tantivy::TantivyDocument {
        doc!(
            self.f_id => session.id.as_str(),
            self.f_title => session.title.as_str(),
            self.f_directory => session.directory.as_str(),
            self.f_agent => session.agent.as_str(),
            self.f_content => session.content.as_str(),
            self.f_timestamp => session.mtime,
            self.f_message_count => session.message_count as i64,
            self.f_mtime => session.mtime,
            self.f_yolo => session.yolo,
        )
    }

    #[allow(dead_code)]
    fn doc_to_session(&self, doc: &tantivy::TantivyDocument) -> Option<Session> {
        let id = doc.get_first(self.f_id)?.as_str()?.to_string();
        let title = doc.get_first(self.f_title)?.as_str()?.to_string();
        let directory = doc.get_first(self.f_directory)?.as_str()?.to_string();
        let agent = doc.get_first(self.f_agent)?.as_str()?.to_string();
        let content = doc.get_first(self.f_content)?.as_str()?.to_string();
        let timestamp_f = doc.get_first(self.f_timestamp)?.as_f64()?;
        let message_count = doc.get_first(self.f_message_count)?.as_i64()? as usize;
        let mtime = doc.get_first(self.f_mtime)?.as_f64()?;
        let yolo = doc.get_first(self.f_yolo)?.as_bool().unwrap_or(false);
        let timestamp = DateTime::from_timestamp(timestamp_f as i64, 0)?.naive_utc();

        Some(Session {
            id,
            agent,
            title,
            directory,
            timestamp,
            content,
            message_count,
            mtime,
            yolo,
        })
    }

    /// Read a string fast field value for a doc_id.
    fn read_str(col: &tantivy::columnar::StrColumn, doc_id: u32, buf: &mut String) -> Option<()> {
        let ord = col.ords().first(doc_id)?;
        buf.clear();
        col.ord_to_str(ord, buf).ok()?;
        Some(())
    }

    /// Get known sessions: id -> (mtime, agent) using fast fields.
    pub fn get_known_sessions(&self) -> HashMap<String, (f64, String)> {
        let searcher = self.searcher();
        let mut known = HashMap::new();
        let mut buf = String::new();

        for segment_reader in searcher.segment_readers() {
            let ff = segment_reader.fast_fields();
            let (Ok(Some(id_col)), Ok(Some(agent_col)), Ok(mtime_col)) =
                (ff.str("id"), ff.str("agent"), ff.f64("mtime"))
            else {
                continue;
            };

            for doc_id in segment_reader.doc_ids_alive() {
                if Self::read_str(&id_col, doc_id, &mut buf).is_none() {
                    continue;
                }
                let id = buf.clone();

                if Self::read_str(&agent_col, doc_id, &mut buf).is_none() {
                    continue;
                }
                let agent = buf.clone();

                let Some(mtime) = mtime_col.first(doc_id) else {
                    continue;
                };

                known.insert(id, (mtime, agent));
            }
        }
        known
    }

    /// Get all sessions metadata from the index using fast fields.
    /// Content is NOT loaded (set to empty string) for performance.
    pub fn get_all_sessions(&self) -> Vec<Session> {
        let searcher = self.searcher();
        let mut sessions = Vec::new();
        let mut buf = String::new();

        for segment_reader in searcher.segment_readers() {
            let ff = segment_reader.fast_fields();
            let (
                Ok(Some(id_col)),
                Ok(Some(title_col)),
                Ok(Some(dir_col)),
                Ok(Some(agent_col)),
                Ok(ts_col),
                Ok(msg_col),
                Ok(mtime_col),
                Ok(yolo_col),
            ) = (
                ff.str("id"),
                ff.str("title"),
                ff.str("directory"),
                ff.str("agent"),
                ff.f64("timestamp"),
                ff.i64("message_count"),
                ff.f64("mtime"),
                ff.bool("yolo"),
            )
            else {
                continue;
            };

            for doc_id in segment_reader.doc_ids_alive() {
                if Self::read_str(&id_col, doc_id, &mut buf).is_none() {
                    continue;
                }
                let id = buf.clone();

                if Self::read_str(&title_col, doc_id, &mut buf).is_none() {
                    continue;
                }
                let title = buf.clone();

                if Self::read_str(&dir_col, doc_id, &mut buf).is_none() {
                    continue;
                }
                let directory = buf.clone();

                if Self::read_str(&agent_col, doc_id, &mut buf).is_none() {
                    continue;
                }
                let agent = buf.clone();

                let Some(timestamp_f) = ts_col.first(doc_id) else {
                    continue;
                };
                let Some(message_count) = msg_col.first(doc_id) else {
                    continue;
                };
                let Some(mtime) = mtime_col.first(doc_id) else {
                    continue;
                };
                let yolo = yolo_col.first(doc_id).unwrap_or(false);

                let Some(timestamp) =
                    DateTime::from_timestamp(timestamp_f as i64, 0).map(|dt| dt.naive_utc())
                else {
                    continue;
                };

                sessions.push(Session {
                    id,
                    agent,
                    title,
                    directory,
                    timestamp,
                    content: String::new(),
                    message_count: message_count as usize,
                    mtime,
                    yolo,
                });
            }
        }
        sessions
    }

    /// Load content for a single session by ID (from stored fields).
    pub fn get_session_content(&self, id: &str) -> Option<String> {
        let searcher = self.searcher();
        let query = TermQuery::new(
            Term::from_field_text(self.f_id, id),
            IndexRecordOption::Basic,
        );
        let results = searcher.search(&query, &TopDocs::with_limit(1)).ok()?;
        let (_, addr) = results.into_iter().next()?;
        let doc = searcher.doc::<tantivy::TantivyDocument>(addr).ok()?;
        doc.get_first(self.f_content)?
            .as_str()
            .map(|s| s.to_string())
    }

    /// Add sessions to the index.
    #[allow(dead_code)]
    pub fn add_sessions(&self, sessions: &[Session]) {
        if sessions.is_empty() {
            return;
        }
        let Some(mut writer) = self.acquire_writer() else {
            return;
        };
        for session in sessions {
            writer.add_document(self.session_to_doc(session)).ok();
        }
        let _ = writer.commit();
        self.reload_reader();
    }

    /// Update sessions (delete + re-add atomically).
    pub fn update_sessions(&self, sessions: &[Session]) {
        if sessions.is_empty() {
            return;
        }
        let Some(mut writer) = self.acquire_writer() else {
            return;
        };
        for session in sessions {
            let term = Term::from_field_text(self.f_id, &session.id);
            writer.delete_term(term);
            writer.add_document(self.session_to_doc(session)).ok();
        }
        let _ = writer.commit();
        self.reload_reader();
    }

    /// Delete sessions by ID.
    pub fn delete_sessions(&self, ids: &[String]) {
        if ids.is_empty() {
            return;
        }
        let Some(mut writer) = self.acquire_writer() else {
            return;
        };
        for id in ids {
            let term = Term::from_field_text(self.f_id, id);
            writer.delete_term(term);
        }
        let _ = writer.commit();
        self.reload_reader();
    }

    /// Batch update: delete IDs + upsert sessions in a single writer + single commit.
    pub fn batch_update(&self, delete_ids: &[String], upsert: &[Session]) {
        if delete_ids.is_empty() && upsert.is_empty() {
            return;
        }
        let Some(mut writer) = self.acquire_writer() else {
            return;
        };
        for id in delete_ids {
            let term = Term::from_field_text(self.f_id, id);
            writer.delete_term(term);
        }
        for session in upsert {
            let term = Term::from_field_text(self.f_id, &session.id);
            writer.delete_term(term);
            writer.add_document(self.session_to_doc(session)).ok();
        }
        let _ = writer.commit();
        self.reload_reader();
    }

    /// Get session count, optionally filtered by agent.
    #[allow(dead_code)]
    pub fn get_session_count(&self, agent_filter: Option<&str>) -> usize {
        let searcher = self.searcher();
        let query: Box<dyn tantivy::query::Query> = match agent_filter {
            Some(agent) => Box::new(TermQuery::new(
                Term::from_field_text(self.f_agent, agent),
                IndexRecordOption::Basic,
            )),
            None => Box::new(AllQuery),
        };
        let all_docs = TopDocs::with_limit(1_000_000);
        searcher
            .search(&*query, &all_docs)
            .map(|r| r.len())
            .unwrap_or(0)
    }

    /// Search sessions with text query and filters.
    pub fn search(
        &self,
        query_text: &str,
        agent_filter: Option<&Filter>,
        directory_filter: Option<&Filter>,
        date_filter: Option<&DateFilter>,
        limit: usize,
    ) -> Vec<(String, f64)> {
        let searcher = self.searcher();
        let index = self.index.as_ref().unwrap();

        let mut must_clauses: Vec<(Occur, Box<dyn tantivy::query::Query>)> = Vec::new();

        // Text query (hybrid exact + fuzzy)
        if !query_text.is_empty()
            && let Some(q) = self.build_hybrid_query(index, query_text)
        {
            must_clauses.push((Occur::Must, q));
        }

        // Agent filter
        if let Some(filter) = agent_filter
            && let Some(q) = self.build_agent_filter(filter)
        {
            must_clauses.push((Occur::Must, q));
        }

        // Directory filter
        if let Some(filter) = directory_filter
            && let Some(q) = self.build_directory_filter(filter)
        {
            must_clauses.push((Occur::Must, q));
        }

        // Date filter
        if let Some(filter) = date_filter
            && let Some(q) = self.build_date_filter(filter)
        {
            must_clauses.push((Occur::Must, q));
        }

        let final_query: Box<dyn tantivy::query::Query> = if must_clauses.is_empty() {
            Box::new(AllQuery)
        } else if must_clauses.len() == 1 {
            must_clauses.pop().unwrap().1
        } else {
            Box::new(BooleanQuery::new(must_clauses))
        };

        // Use BM25 relevance scoring when there's a query, time sort otherwise.
        // Read IDs from fast fields (column-oriented) instead of stored fields.
        if query_text.is_empty() {
            let collector =
                TopDocs::with_limit(limit).order_by_fast_field::<f64>("timestamp", Order::Desc);
            match searcher.search(&*final_query, &collector) {
                Ok(results) => {
                    let mut buf = String::new();
                    results
                        .into_iter()
                        .filter_map(|(_, addr)| {
                            let seg = searcher.segment_reader(addr.segment_ord);
                            let id_col = seg.fast_fields().str("id").ok()??;
                            Self::read_str(&id_col, addr.doc_id, &mut buf)?;
                            Some((buf.clone(), 0.0))
                        })
                        .collect()
                }
                Err(_) => vec![],
            }
        } else {
            let collector = TopDocs::with_limit(limit);
            match searcher.search(&*final_query, &collector) {
                Ok(results) => {
                    let mut buf = String::new();
                    results
                        .into_iter()
                        .filter_map(|(score, addr)| {
                            let seg = searcher.segment_reader(addr.segment_ord);
                            let id_col = seg.fast_fields().str("id").ok()??;
                            Self::read_str(&id_col, addr.doc_id, &mut buf)?;
                            Some((buf.clone(), score as f64))
                        })
                        .collect()
                }
                Err(_) => vec![],
            }
        }
    }

    fn build_hybrid_query(
        &self,
        index: &Index,
        query_text: &str,
    ) -> Option<Box<dyn tantivy::query::Query>> {
        // Exact match (BM25) boosted 5x
        let parser = QueryParser::for_index(index, vec![self.f_title, self.f_content]);
        let exact_query = parser.parse_query(query_text).ok()?;
        let boosted_exact = BoostQuery::new(exact_query, 5.0);

        // Fuzzy match per term
        let mut fuzzy_parts: Vec<(Occur, Box<dyn tantivy::query::Query>)> = Vec::new();
        for term_str in query_text.split_whitespace() {
            let fuzzy_title =
                FuzzyTermQuery::new_prefix(Term::from_field_text(self.f_title, term_str), 1, true);
            let fuzzy_content = FuzzyTermQuery::new_prefix(
                Term::from_field_text(self.f_content, term_str),
                1,
                true,
            );
            let term_q = BooleanQuery::new(vec![
                (Occur::Should, Box::new(fuzzy_title)),
                (Occur::Should, Box::new(fuzzy_content)),
            ]);
            fuzzy_parts.push((Occur::Must, Box::new(term_q)));
        }

        let fuzzy_query = BooleanQuery::new(fuzzy_parts);

        // Combine: exact OR fuzzy
        Some(Box::new(BooleanQuery::new(vec![
            (Occur::Should, Box::new(boosted_exact)),
            (Occur::Should, Box::new(fuzzy_query)),
        ])))
    }

    fn build_agent_filter(&self, filter: &Filter) -> Option<Box<dyn tantivy::query::Query>> {
        let mut clauses: Vec<(Occur, Box<dyn tantivy::query::Query>)> = Vec::new();

        if !filter.include.is_empty() {
            if filter.include.len() == 1 {
                clauses.push((
                    Occur::Must,
                    Box::new(TermQuery::new(
                        Term::from_field_text(self.f_agent, &filter.include[0]),
                        IndexRecordOption::Basic,
                    )),
                ));
            } else {
                let terms: Vec<Term> = filter
                    .include
                    .iter()
                    .map(|a| Term::from_field_text(self.f_agent, a))
                    .collect();
                clauses.push((Occur::Must, Box::new(TermSetQuery::new(terms))));
            }
        }

        for excl in &filter.exclude {
            clauses.push((
                Occur::MustNot,
                Box::new(TermQuery::new(
                    Term::from_field_text(self.f_agent, excl),
                    IndexRecordOption::Basic,
                )),
            ));
        }

        if clauses.is_empty() {
            None
        } else {
            let q: Box<dyn tantivy::query::Query> =
                if clauses.len() == 1 && clauses[0].0 == Occur::Must {
                    clauses.pop().unwrap().1
                } else {
                    Box::new(BooleanQuery::new(clauses))
                };
            // Zero-boost: filter only, no score contribution
            Some(Box::new(BoostQuery::new(q, 0.0)))
        }
    }

    fn build_directory_filter(&self, filter: &Filter) -> Option<Box<dyn tantivy::query::Query>> {
        let mut clauses: Vec<(Occur, Box<dyn tantivy::query::Query>)> = Vec::new();

        if !filter.include.is_empty() {
            let mut include_clauses: Vec<(Occur, Box<dyn tantivy::query::Query>)> = Vec::new();
            for pat in &filter.include {
                let escaped = regex::escape(pat);
                let pattern = format!("(?i).*{escaped}.*");
                if let Ok(q) = RegexQuery::from_pattern(&pattern, self.f_directory) {
                    include_clauses.push((Occur::Should, Box::new(q)));
                }
            }
            if !include_clauses.is_empty() {
                clauses.push((Occur::Must, Box::new(BooleanQuery::new(include_clauses))));
            }
        }

        for excl in &filter.exclude {
            let escaped = regex::escape(excl);
            let pattern = format!("(?i).*{escaped}.*");
            if let Ok(q) = RegexQuery::from_pattern(&pattern, self.f_directory) {
                clauses.push((Occur::MustNot, Box::new(q)));
            }
        }

        if clauses.is_empty() {
            None
        } else {
            // Zero-boost: filter only, no score contribution
            Some(Box::new(BoostQuery::new(
                Box::new(BooleanQuery::new(clauses)),
                0.0,
            )))
        }
    }

    fn build_date_filter(&self, filter: &DateFilter) -> Option<Box<dyn tantivy::query::Query>> {
        let cutoff = filter.cutoff.and_utc().timestamp() as f64;

        let range_query: Box<dyn tantivy::query::Query> = match filter.op {
            DateOp::Exact => {
                let end = cutoff + 86400.0;
                Box::new(FastFieldRangeQuery::new(
                    std::ops::Bound::Included(Term::from_field_f64(self.f_timestamp, cutoff)),
                    std::ops::Bound::Excluded(Term::from_field_f64(self.f_timestamp, end)),
                ))
            }
            DateOp::LessThan => {
                // Newer than cutoff
                Box::new(FastFieldRangeQuery::new(
                    std::ops::Bound::Included(Term::from_field_f64(self.f_timestamp, cutoff)),
                    std::ops::Bound::Unbounded,
                ))
            }
            DateOp::GreaterThan => {
                // Older than cutoff
                Box::new(FastFieldRangeQuery::new(
                    std::ops::Bound::Unbounded,
                    std::ops::Bound::Excluded(Term::from_field_f64(self.f_timestamp, cutoff)),
                ))
            }
        };

        if filter.negated {
            Some(Box::new(BooleanQuery::new(vec![
                (Occur::Must, Box::new(AllQuery)),
                (Occur::MustNot, range_query),
            ])))
        } else {
            Some(range_query)
        }
    }

    /// Wipe the index and rebuild from scratch.
    pub fn clear(&self) {
        if let Some(mut writer) = self.acquire_writer() {
            writer.delete_all_documents().ok();
            let _ = writer.commit();
            self.reload_reader();
        }
    }
}
