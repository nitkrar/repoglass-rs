//! Post-retrieval reranking: coherence boost, then query boost, then
//! path penalties and saturation over the top k.

use super::boosting::{boost_coherence, boost_query, Candidate, Definitions, NonCandidateLoader};
use super::penalties::select_top;
use crate::config::Settings;

pub fn rerank(mut candidates: Vec<Candidate>, query: &str, settings: &Settings,
              loader: Option<&mut NonCandidateLoader>, penalise_paths: bool,
              limit: usize) -> anyhow::Result<Vec<Candidate>> {
    if candidates.is_empty() {
        return Ok(candidates);
    }
    if settings.file_coherence.get() > 0.0 {
        boost_coherence(&mut candidates, settings);
    }
    let mut defs = Definitions::new();
    boost_query(&mut candidates, query, settings, loader, &mut defs)?;
    Ok(select_top(candidates, settings, limit, penalise_paths))
}
