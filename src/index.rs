use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use chrono::DateTime;
use tantivy::collector::TopDocs;
use tantivy::query::{
    AllQuery, BooleanQuery, BoostQuery, FuzzyTermQuery, Occur, QueryParser,
    FastFieldRangeQuery, RegexQuery, TermQuery, TermSetQuery,
};
use tantivy::schema::*;
use tantivy::{
    doc, Index, IndexReader, IndexWriter, Order, ReloadPolicy, Searcher, Term,
};

use crate::config;
use crate::query::{DateFilter, DateOp, Filter};
use crate::session::Session;

const SCHEMA_VERSION: u32 = 21; // Bumped for Rust version
const LOCK_FILE: &str = ".tantivy-writer.lock";
const WRITER_MEMORY: usize = 50_000_000; // 50MB

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
        let index_path = config::index_dir();

        let mut builder = Schema::builder();
        let f_id = builder.add_text_field("id", STRING | STORED);
        let f_title = builder.add_text_field("title", TEXT | STORED);
        let f_directory = builder.add_text_field("directory", STRING | STORED);
        let f_agent = builder.add_text_field("agent", STRING | STORED);
        let f_content = builder.add_text_field("content", TEXT | STORED);
        let f_timestamp = builder.add_f64_field(
            "timestamp",
            NumericOptions::default()
                .set_indexed()
                .set_stored()
                .set_fast(),
        );
        let f_message_count = builder.add_i64_field("message_count", STORED | INDEXED);
        let f_mtime = builder.add_f64_field("mtime", STORED);
        let f_yolo = builder.add_bool_field("yolo", STORED);
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

    /// Get known sessions: id -> (mtime, agent)
    pub fn get_known_sessions(&self) -> HashMap<String, (f64, String)> {
        let searcher = self.searcher();
        let mut known = HashMap::new();

        let all_docs = TopDocs::with_limit(1_000_000);
        if let Ok(results) = searcher.search(&AllQuery, &all_docs) {
            for (_score, addr) in results {
                if let Ok(doc) = searcher.doc::<tantivy::TantivyDocument>(addr)
                    && let (Some(id), Some(mtime), Some(agent)) = (
                        doc.get_first(self.f_id).and_then(|v| v.as_str()),
                        doc.get_first(self.f_mtime).and_then(|v| v.as_f64()),
                        doc.get_first(self.f_agent).and_then(|v| v.as_str()),
                    ) {
                        known.insert(id.to_string(), (mtime, agent.to_string()));
                    }
            }
        }
        known
    }

    /// Get all sessions from the index.
    pub fn get_all_sessions(&self) -> Vec<Session> {
        let searcher = self.searcher();
        let mut sessions = Vec::new();

        let all_docs = TopDocs::with_limit(1_000_000);
        if let Ok(results) = searcher.search(&AllQuery, &all_docs) {
            for (_score, addr) in results {
                if let Ok(doc) = searcher.doc::<tantivy::TantivyDocument>(addr)
                    && let Some(session) = self.doc_to_session(&doc) {
                        sessions.push(session);
                    }
            }
        }
        sessions
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
        sort_by_time: bool,
    ) -> Vec<(String, f64)> {
        let searcher = self.searcher();
        let index = self.index.as_ref().unwrap();

        let mut must_clauses: Vec<(Occur, Box<dyn tantivy::query::Query>)> = Vec::new();

        // Text query (hybrid exact + fuzzy)
        if !query_text.is_empty() && !sort_by_time
            && let Some(q) = self.build_hybrid_query(index, query_text) {
                must_clauses.push((Occur::Must, q));
            }

        // Agent filter
        if let Some(filter) = agent_filter
            && let Some(q) = self.build_agent_filter(filter) {
                must_clauses.push((Occur::Must, q));
            }

        // Directory filter
        if let Some(filter) = directory_filter
            && let Some(q) = self.build_directory_filter(filter) {
                must_clauses.push((Occur::Must, q));
            }

        // Date filter
        if let Some(filter) = date_filter
            && let Some(q) = self.build_date_filter(filter) {
                must_clauses.push((Occur::Must, q));
            }

        let final_query: Box<dyn tantivy::query::Query> = if must_clauses.is_empty() {
            Box::new(AllQuery)
        } else if must_clauses.len() == 1 {
            must_clauses.pop().unwrap().1
        } else {
            Box::new(BooleanQuery::new(must_clauses))
        };

        // Sort by time or relevance
        if sort_by_time || query_text.is_empty() {
            let collector =
                TopDocs::with_limit(limit).order_by_fast_field::<f64>("timestamp", Order::Desc);
            match searcher.search(&*final_query, &collector) {
                Ok(results) => results
                    .into_iter()
                    .filter_map(|(_, addr)| {
                        let doc = searcher.doc::<tantivy::TantivyDocument>(addr).ok()?;
                        let id = doc.get_first(self.f_id)?.as_str()?.to_string();
                        Some((id, 0.0))
                    })
                    .collect(),
                Err(_) => vec![],
            }
        } else {
            let collector = TopDocs::with_limit(limit);
            match searcher.search(&*final_query, &collector) {
                Ok(results) => results
                    .into_iter()
                    .filter_map(|(score, addr)| {
                        let doc = searcher.doc::<tantivy::TantivyDocument>(addr).ok()?;
                        let id = doc.get_first(self.f_id)?.as_str()?.to_string();
                        Some((id, score as f64))
                    })
                    .collect(),
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
        } else if clauses.len() == 1 && clauses[0].0 == Occur::Must {
            Some(clauses.pop().unwrap().1)
        } else {
            Some(Box::new(BooleanQuery::new(clauses)))
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
            Some(Box::new(BooleanQuery::new(clauses)))
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
        }
    }
}
