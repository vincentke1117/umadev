//! Unified retrieval entry point — picks the configured engine and returns
//! ranked chunks ready for prompt formatting or TUI display.
//!
//! This is the single function the agent crate calls, replacing the old
//! `phase_knowledge_digest` / `knowledge_top_files` internals. It decides:
//! 1. Which `knowledge/` subdirs are relevant for the current phase.
//! 2. Whether to use lexical BM25 or the default hybrid BM25+local-vector
//!    channel (which degrades to BM25 when the verified model is unavailable).
//! 3. RRF (Reciprocal Rank Fusion) to merge the two rankings when hybrid.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use umadev_spec::Phase;

use crate::chunker::Chunk;
use crate::corpus::{knowledge_roots, CorpusSet};
use crate::index::{load_or_build_index_corpus, Bm25Index};
use crate::vector;

/// A retrieval hit: the chunk + a normalised 0..1 score.
#[derive(Debug, Clone)]
pub struct ScoredChunk {
    /// The matched chunk.
    pub chunk: Chunk,
    /// Normalised score in 0.0..=1.0 (1.0 = best match in this query).
    pub score: f32,
}

/// Which retrieval engine to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum RetrievalEngine {
    /// BM25 keyword retrieval only — offline and zero-dependency. Opt in for
    /// lexical-only retrieval; otherwise `Hybrid` degrades to this when no
    /// embedding backend is available.
    Bm25,
    /// BM25 + vector RRF fusion — the default. The local candle model is tried
    /// first; explicitly authorized cloud embedding is optional. With neither,
    /// this behaves identically to `Bm25`.
    #[default]
    Hybrid,
}

/// Per-project retrieval configuration (mirrors `[knowledge]` in .umadevrc).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct RetrievalConfig {
    /// Whether the knowledge base is enabled at all.
    pub enabled: bool,
    /// Which engine to use.
    pub engine: RetrievalEngine,
    /// How many chunks to return per query.
    pub top_k: usize,
    /// Extra directories (relative to project root) to include alongside
    /// the built-in `knowledge/` tree.
    pub custom_dirs: Vec<String>,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            engine: RetrievalEngine::default(),
            // The seeded standards library grew large (35+ focused standards,
            // hundreds of chunks). A multi-feature project legitimately needs
            // several at once (e.g. layering + API + auth + payment + the
            // platform/framework standard), so the per-phase digest returns the
            // top 12 ranked chunks — relevance still decides which win, but
            // enough land that no major applicable standard gets crowded out.
            // Tune per project via `[knowledge] top_k` in `.umadevrc`.
            top_k: 12,
            custom_dirs: Vec::new(),
        }
    }
}

/// Map a pipeline phase to the knowledge subdirectories most relevant to
/// it. Mirrors the legacy `phase_knowledge_digest` mapping (phases.rs:64)
/// so phase-aware filtering behaviour is preserved.
///
/// **These are UmaDev's built-in business assumptions** about which
/// knowledge folders each pipeline phase should consult (e.g. Docs reads
/// `experts/product-manager` + `experts/architect` + `experts/uiux-designer`).
/// They encode the default knowledge-base layout shipped with UmaDev.
/// Teams whose `knowledge/` tree uses different directory names have two
/// non-fork escape hatches:
/// - per-phase override via `UMADEV_KNOWLEDGE_PHASE_SUBDIRS` (full
///   replacement for a specific phase), and/or
/// - global extras via `UMADEV_KNOWLEDGE_EXTRA_SUBDIRS` (appended to
///   every phase).
///
/// If a phase filter finds nothing, `filter_by_phase` warns + falls back to
/// unfiltered top-k so the prompt still gets context.
#[must_use]
pub fn phase_subdirs(phase: Phase) -> &'static [&'static str] {
    match phase {
        Phase::Research => &[], // research scans the whole tree
        Phase::Docs => &[
            "experts/product-manager",
            "experts/architect",
            "experts/uiux-designer",
            "product",
            "architecture",
            "design",
            "frontend",
            "industries",
            // So the architecture doc can choose + standardize the target
            // platform (web / mobile / desktop / mini-program / HarmonyOS).
            "mobile",
            "desktop",
            "miniprogram",
            "harmony",
            "cross-platform",
        ],
        Phase::DocsConfirm | Phase::PreviewConfirm => &[],
        Phase::Spec => &[
            "experts/product-manager",
            "experts/architect",
            "development",
            "00-governance",
            "product",
        ],
        Phase::Frontend => &[
            "experts/frontend-lead",
            "experts/uiux-designer",
            "frontend",
            "design",
            // NOTE: `design-systems` is intentionally NOT retrieved here. The
            // CHOSEN archetype + the full anti-AI-slop rules are inlined as the
            // binding design contract (see coach::load_design_system_inject), so
            // BM25-retrieving the dir would only duplicate that content and risk
            // surfacing a DIFFERENT archetype's chunks that conflict with the
            // bound one.
            "seed-templates",
            // Multi-platform client standards — the "frontend" phase builds the
            // CLIENT, which may be web, mobile, desktop, mini-program, or
            // HarmonyOS. The relevant platform standard injects by BM25 once the
            // architecture doc declares the target platform.
            "mobile",
            "desktop",
            "miniprogram",
            "harmony",
            "cross-platform",
        ],
        Phase::Backend => &[
            "experts/backend-lead",
            "experts/architect",
            "backend",
            "api",
            "database",
            "security",
            "cloud-native",
        ],
        Phase::Quality => &[
            "experts/qa-lead",
            "experts/architect",
            "testing",
            "security",
            "performance",
            "observability",
            "00-governance",
        ],
        Phase::Delivery => &[
            "experts/devops",
            "cicd",
            "operations",
            "release-engineering",
            "compliance",
            "00-governance",
            "security",
        ],
    }
}

/// Compatibility projection of the path-based retrieval corpus. New product
/// callers should retain [`corpus_set`] (or their discovered [`CorpusSet`]) and
/// pass that same value to `retrieve_corpus*` and vector construction; reducing
/// it to paths discards provenance.
#[must_use]
pub fn corpus_dirs(project_root: &Path, knowledge_dir: &Path) -> Vec<PathBuf> {
    knowledge_roots(project_root, Some(knowledge_dir), &[]).paths()
}

/// The exact provenance-aware source set used by lexical retrieval and vector
/// indexing. Project knowledge is additive to bundled curated knowledge.
#[must_use]
pub fn corpus_set(
    project_root: &Path,
    knowledge_dir: &Path,
    config: &RetrievalConfig,
) -> CorpusSet {
    if !config.enabled {
        return CorpusSet::empty();
    }
    knowledge_roots(project_root, Some(knowledge_dir), &config.custom_dirs)
}

/// Run a retrieval query against the project's knowledge base.
///
/// Builds (or loads the cached) BM25 index, optionally queries the vector
/// store, fuses the rankings via RRF, and returns the top-K chunks with
/// normalised scores.
///
/// `project_root` is the workspace root (where `.umadev/` lives).
/// `knowledge_dir` is the `knowledge/` directory (usually `project_root/knowledge`).
///
/// Returns an empty vec when disabled, the index is empty, or the query
/// yields no matches — never errors (fail-open).
#[must_use]
pub fn retrieve(
    project_root: &Path,
    knowledge_dir: &Path,
    config: &RetrievalConfig,
    query: &str,
    phase: Phase,
) -> Vec<ScoredChunk> {
    retrieve_with_vector(project_root, knowledge_dir, config, query, phase, None)
}

/// Retrieval with a pre-embedded query vector. This is the **real hybrid
/// path**: when `query_vec` is `Some` AND the vector store is populated,
/// BM25 and vector rankings are fused via RRF (k=60). When `query_vec` is
/// `None` or the store is empty, this is identical to pure BM25.
///
/// The query vector must be obtained **asynchronously** (via
/// [`vector::embed_query`]) by the caller — typically the async runner
/// pre-embeds the requirement once, then passes `Some(&qvec)` into the
/// sync render functions. This keeps the network call isolated to the
/// runner seam and fail-open (a `None` vector just means BM25 only).
#[must_use]
pub fn retrieve_with_vector(
    project_root: &Path,
    knowledge_dir: &Path,
    config: &RetrievalConfig,
    query: &str,
    phase: Phase,
    query_vec: Option<&[f32]>,
) -> Vec<ScoredChunk> {
    retrieve_with_vector_and_expansion(
        project_root,
        knowledge_dir,
        config,
        query,
        phase,
        query_vec,
        None,
    )
}

/// Retrieval with an optional HyDE-style query EXPANSION — the BM25-first
/// answer to lexical mismatch.
///
/// BM25 only matches terms the user literally wrote; a requirement phrased in
/// the user's words often shares few tokens with the curated docs that answer
/// it. When `expansion` is `Some` (a base-generated *hypothetical answer /
/// relevant code passage* for the requirement), its BM25 ranking is computed
/// alongside the original query's and the two are RANK-FUSED via RRF (k=60).
/// The hypothetical, being written in the *answer's* vocabulary, recalls
/// docs the bare query would miss; fusing (rather than replacing) keeps the
/// query's own exact matches in the running too.
///
/// Fail-open / additive: `expansion = None` (or an empty/whitespace string, or
/// an expansion that matches nothing) is byte-for-byte identical to
/// [`retrieve_with_vector`]. The hypothetical-answer GENERATION lives in the
/// agent crate (where the base driver is); this crate only fuses the result.
#[must_use]
pub fn retrieve_with_vector_and_expansion(
    project_root: &Path,
    knowledge_dir: &Path,
    config: &RetrievalConfig,
    query: &str,
    phase: Phase,
    query_vec: Option<&[f32]>,
    expansion: Option<&str>,
) -> Vec<ScoredChunk> {
    if !config.enabled || query.trim().is_empty() {
        return Vec::new();
    }

    let corpus = corpus_set(project_root, knowledge_dir, config);
    retrieve_corpus_with_vector_and_expansion(
        project_root,
        &corpus,
        config,
        query,
        phase,
        query_vec,
        expansion,
    )
}

/// Retrieve from an already-discovered corpus. Product callers use this seam so
/// lexical retrieval, vector indexing, previews, and digests consume the exact
/// same ordered source set rather than rediscovering roots independently.
#[must_use]
pub fn retrieve_corpus(
    project_root: &Path,
    corpus: &CorpusSet,
    config: &RetrievalConfig,
    query: &str,
    phase: Phase,
) -> Vec<ScoredChunk> {
    retrieve_corpus_with_vector(project_root, corpus, config, query, phase, None)
}

/// [`retrieve_corpus`] with a pre-embedded query vector.
#[must_use]
pub fn retrieve_corpus_with_vector(
    project_root: &Path,
    corpus: &CorpusSet,
    config: &RetrievalConfig,
    query: &str,
    phase: Phase,
    query_vec: Option<&[f32]>,
) -> Vec<ScoredChunk> {
    retrieve_corpus_with_vector_and_expansion(
        project_root,
        corpus,
        config,
        query,
        phase,
        query_vec,
        None,
    )
}

/// Provenance-aware retrieval with optional vector and HyDE channels.
#[must_use]
pub fn retrieve_corpus_with_vector_and_expansion(
    project_root: &Path,
    corpus: &CorpusSet,
    config: &RetrievalConfig,
    query: &str,
    phase: Phase,
    query_vec: Option<&[f32]>,
    expansion: Option<&str>,
) -> Vec<ScoredChunk> {
    if !config.enabled || query.trim().is_empty() || corpus.is_empty() {
        return Vec::new();
    }

    // Build/load the BM25 index over the caller's exact canonical,
    // provenance-aware set. Vector construction receives this same set.
    let index = load_or_build_index_corpus(project_root, corpus);
    if index.chunks.is_empty() {
        return Vec::new();
    }

    // BM25 results over the full index (over-fetch so RRF has room).
    // Query-side cleaning: drop low-IDF / function-word tokens so the rare,
    // on-topic terms dominate the ranking instead of being diluted by filler
    // (and, for CJK bigram queries, by a flood of weak near-matches). The mask
    // is fail-open — if it would empty the query it returns the raw tokens — and
    // we additionally fall back to the unmasked search if the masked search
    // somehow finds nothing, so masking can only ever HELP, never starve.
    let over_fetch = config.top_k.saturating_mul(3);
    let masked_terms = index.mask_low_idf_terms(query, idf_floor());
    let bm25_masked = index.search_terms(&masked_terms, over_fetch);
    let query_bigram = if bm25_masked.is_empty() {
        index.search(query, over_fetch)
    } else {
        bm25_masked
    };
    let query_bm25 = fuse_trigram_channel(&index, query, query_bigram, over_fetch);
    // Preserve the untouched query as the primary channel, then add a bounded
    // bilingual alias channel. Literal matches receive both RRF contributions;
    // alias-only matches receive one, so inferred cross-language relevance can
    // recover a miss without outranking stronger current-query evidence.
    let expanded_query = crate::query_expansion::expand_bilingual_query(query);
    let query_bm25 = if expanded_query == query {
        query_bm25
    } else {
        let expanded_terms = index.mask_low_idf_terms(&expanded_query, idf_floor());
        let expanded = index.search_terms(&expanded_terms, over_fetch);
        if expanded.is_empty() {
            query_bm25
        } else {
            rrf_fuse_bm25(&query_bm25, &expanded, RRF_K, over_fetch)
        }
    };
    // HyDE fusion: when a hypothetical-answer expansion is present and itself
    // matches something, RRF-fuse its ranking with the query's. Empty / no-match
    // expansion → identity (just the query ranking), preserving prior behaviour.
    let bm25_raw = match expansion {
        Some(exp) if !exp.trim().is_empty() => {
            let exp_bm25 = index.search(exp, over_fetch);
            if exp_bm25.is_empty() {
                query_bm25
            } else {
                rrf_fuse_bm25(&query_bm25, &exp_bm25, RRF_K, over_fetch)
            }
        }
        _ => query_bm25,
    };
    // Metadata is a separate candidate-generation channel, not merely a boost
    // applied after lexical truncation. This matters when a document title/tag
    // names the user's concept but its body uses another language or vocabulary.
    // Fuse at rank level so metadata cannot replace lexical evidence; when both
    // channels select the same document chunk their agreement is rewarded.
    let metadata = metadata_rank(&index, &expanded_query, &bm25_raw, over_fetch);
    let bm25_raw = if metadata.is_empty() {
        bm25_raw
    } else {
        rrf_fuse_bm25(&bm25_raw, &metadata, RRF_K, over_fetch)
    };
    let bm25_hits = filter_by_phase(&index, &bm25_raw, phase, over_fetch);

    // Vector fusion only when: hybrid engine, vector layer enabled, a query
    // vector was provided, AND the store actually has vectors. Whichever ranked
    // list we end up with, it flows through ONE unified post-rank ([`normalise`]
    // + [`dedup_learned_chunks`]) so duplicate sedimented lessons can't be
    // injected twice (see the dedup rationale below).
    let use_vector =
        config.engine == RetrievalEngine::Hybrid && vector::is_enabled() && query_vec.is_some();
    let ranked = if use_vector {
        let query_vec = query_vec.unwrap_or(&[]);
        let store = vector::VectorStore::load(project_root);
        // MED #4: only fuse when the store was built over the SAME chunk-position
        // mapping as the live index. BM25 rebuilds lazily at query time while the
        // vector store rebuilds separately (async), so a corpus changed since the
        // store was built shifts `chunk_idx` — a stale-yet-in-range hit would then
        // attribute to the WRONG chunk. A fingerprint mismatch (or an unstamped /
        // legacy store) skips vector fusion (BM25-only) until the store is rebuilt.
        let store_aligned =
            !store.is_empty() && store.corpus_sig() == crate::index::corpus_fingerprint(&index);
        // P0-2: collision-safe — fuse on the store's `chunk_idx`, not the lossy
        // `(path, section)` remap that silently dropped legitimate colliding hits.
        let vec_hits = if store_aligned {
            store.search_with_idx(query_vec, over_fetch)
        } else {
            Vec::new()
        };
        if vec_hits.is_empty() {
            bm25_hits
        } else {
            // Fuse the OVER-FETCHED, UNFILTERED BM25 list with the vector list so
            // both channels contribute symmetrically before truncation (#7
            // de-bias), THEN run the fused ranking through `filter_by_phase` so the
            // vector channel can NOT reintroduce off-phase chunks the BM25 filter
            // excludes — e.g. a `design-systems` chunk in the Frontend phase (MED
            // #2). Final top-k truncation happens after document diversification.
            let fused = rrf_fuse(&index, &bm25_raw, &vec_hits, RRF_K, over_fetch);
            let fused = filter_by_phase(&index, &fused, phase, over_fetch);
            if fused.is_empty() {
                bm25_hits
            } else {
                fused
            }
        }
    } else {
        bm25_hits
    };
    // Apply bounded metadata relevance, then blend the cross-project usefulness
    // prior. A missing/corrupt store is neutral and never blocks retrieval.
    let ranked = rerank_by_metadata(&index, &expanded_query, ranked);
    let usefulness = crate::usefulness::UsefulnessStore::load();
    diversify_paths(
        dedup_learned_chunks(normalise(&index, ranked, &usefulness)),
        config.top_k,
    )
}

const MAX_QUERY_FIELD_TERMS: usize = 64;
const MAX_FIELD_CHARS: usize = 2_048;
const MAX_FIELD_TOKENS: usize = 128;
const MAX_METADATA_BOOST: f64 = 0.5;
const USEFULNESS_BLEND: f32 = 0.1;

fn corpus_origin_weight(origin: crate::CorpusOrigin) -> f32 {
    match origin {
        // Project-authored policy is the team's local source of truth and must
        // not be crowded out by a larger bundled library on an otherwise close
        // match. The boost is deliberately bounded: materially stronger current
        // evidence still wins.
        crate::CorpusOrigin::ProjectCustom => 1.2,
        crate::CorpusOrigin::ProjectSkillPackage => 1.1,
        crate::CorpusOrigin::ProjectLearned => 1.05,
        crate::CorpusOrigin::Unknown
        | crate::CorpusOrigin::BundledCurated
        | crate::CorpusOrigin::GlobalSafeLearned => 1.0,
    }
}

#[derive(Default)]
struct MetadataTokens {
    title: BTreeSet<String>,
    section: BTreeSet<String>,
    tags: BTreeSet<String>,
    domain: BTreeSet<String>,
    path: BTreeSet<String>,
}

impl MetadataTokens {
    fn from_chunk(chunk: &Chunk) -> Self {
        let tags = chunk
            .meta
            .tags
            .iter()
            .take(MAX_FIELD_TOKENS)
            .flat_map(|tag| bounded_tokens(tag, MAX_FIELD_TOKENS))
            .take(MAX_FIELD_TOKENS)
            .collect();
        Self {
            title: bounded_tokens(&chunk.meta.title, MAX_FIELD_TOKENS),
            section: bounded_tokens(&chunk.meta.section, MAX_FIELD_TOKENS),
            tags,
            domain: bounded_tokens(&chunk.meta.domain, MAX_FIELD_TOKENS),
            path: bounded_tokens(&chunk.meta.path, MAX_FIELD_TOKENS),
        }
    }

    fn term_weight(&self, term: &str) -> f64 {
        if self.title.contains(term) {
            3.0
        } else if self.tags.contains(term) {
            2.5
        } else if self.section.contains(term) {
            1.75
        } else if self.domain.contains(term) {
            1.25
        } else if self.path.contains(term) {
            1.0
        } else {
            0.0
        }
    }
}

/// Rank at most one representative chunk per document using bounded metadata.
///
/// The representative is the document's strongest existing lexical candidate
/// when possible, making agreement between channels address the same chunk id.
/// Otherwise its first chunk is used, allowing a title/tag-only document into
/// the over-fetched pool. IDF is computed over documents (not chunks), so a long
/// document cannot dilute a term merely because its metadata is copied onto many
/// sections.
fn metadata_rank(
    index: &Bm25Index,
    query: &str,
    lexical: &[(usize, f64)],
    limit: usize,
) -> Vec<(usize, f64)> {
    if limit == 0 {
        return Vec::new();
    }
    let query_terms = bounded_metadata_query_tokens(query);
    if query_terms.is_empty() {
        return Vec::new();
    }
    let lexical_rank = lexical
        .iter()
        .enumerate()
        .map(|(rank, (idx, _))| (*idx, rank))
        .collect::<HashMap<_, _>>();
    let mut documents = HashMap::<String, (usize, usize, MetadataTokens)>::new();
    for (idx, chunk) in index.chunks.iter().enumerate() {
        let rank = lexical_rank.get(&idx).copied().unwrap_or(usize::MAX);
        let entry = documents
            .entry(chunk.meta.path.clone())
            .or_insert_with(|| (idx, rank, MetadataTokens::from_chunk(chunk)));
        if rank < entry.1 {
            entry.0 = idx;
            entry.1 = rank;
        }
    }
    if documents.is_empty() {
        return Vec::new();
    }
    let document_count = documents.len() as f64;
    let term_idf = query_terms
        .iter()
        .map(|term| {
            let frequency = documents
                .values()
                .filter(|(_, _, profile)| profile.term_weight(term) > 0.0)
                .count() as f64;
            let idf = ((document_count - frequency + 0.5) / (frequency + 0.5) + 1.0).ln();
            (term, idf)
        })
        .collect::<Vec<_>>();
    let mut ranked = documents
        .into_values()
        .filter_map(|(idx, _, profile)| {
            let mut contributions = term_idf
                .iter()
                .filter_map(|(term, idf)| {
                    let weight = profile.term_weight(term);
                    (weight > 0.0).then_some(weight * *idf)
                })
                .collect::<Vec<_>>();
            contributions.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
            let score = contributions.into_iter().take(3).sum::<f64>();
            if score <= f64::EPSILON || !score.is_finite() {
                return None;
            }
            score.is_finite().then_some((idx, score))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    ranked.truncate(limit);
    ranked
}

fn bounded_tokens(text: &str, max_tokens: usize) -> BTreeSet<String> {
    let end = text
        .char_indices()
        .nth(MAX_FIELD_CHARS)
        .map_or(text.len(), |(index, _)| index);
    crate::tokenizer::tokenize(&text[..end])
        .into_iter()
        .take(max_tokens)
        .collect()
}

fn bounded_metadata_query_tokens(text: &str) -> BTreeSet<String> {
    let all = bounded_tokens(text, MAX_QUERY_FIELD_TERMS);
    let phrases = all
        .iter()
        .filter(|term| term.chars().count() > 1)
        .cloned()
        .collect::<BTreeSet<_>>();
    if phrases.is_empty() {
        all
    } else {
        phrases
    }
}

/// Blend a bounded BM25F-style metadata signal into the lexical/vector rank.
/// Each query term contributes at most its strongest field, so duplicating a
/// word across title, tags, and path cannot manufacture relevance. Candidate-
/// local IDF suppresses generic metadata while exact title/tag/path matches
/// remain useful. Up to three distinct matches establish confidence, and the
/// additive contribution is capped at half of the strongest lexical score;
/// body relevance remains the primary channel, while an exact title/tag match
/// can still recover a document whose body uses different vocabulary.
fn rerank_by_metadata(
    index: &Bm25Index,
    query: &str,
    hits: Vec<(usize, f64)>,
) -> Vec<(usize, f64)> {
    if hits.len() < 2 {
        return hits;
    }
    let query_terms = bounded_metadata_query_tokens(query);
    if query_terms.is_empty() || hits.iter().any(|(_, score)| !score.is_finite()) {
        return hits;
    }
    let Some(profiles) = hits
        .iter()
        .map(|(idx, _)| index.chunks.get(*idx).map(MetadataTokens::from_chunk))
        .collect::<Option<Vec<_>>>()
    else {
        return hits;
    };
    let count = profiles.len() as f64;
    let strongest_lexical = hits.iter().map(|(_, score)| *score).fold(0.0_f64, f64::max);
    if strongest_lexical <= f64::EPSILON || !strongest_lexical.is_finite() {
        return hits;
    }
    let term_idf = query_terms
        .iter()
        .map(|term| {
            let document_frequency = profiles
                .iter()
                .filter(|candidate| candidate.term_weight(term) > 0.0)
                .count() as f64;
            let idf = ((count - document_frequency + 0.5) / (document_frequency + 0.5) + 1.0).ln();
            (term, idf)
        })
        .collect::<Vec<_>>();
    let evidence = profiles
        .iter()
        .map(|profile| {
            let mut contributions = term_idf
                .iter()
                .filter_map(|(term, idf)| {
                    let weight = profile.term_weight(term);
                    (weight > 0.0).then_some(weight * *idf)
                })
                .collect::<Vec<_>>();
            let matched = contributions.len();
            contributions.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
            (contributions.into_iter().take(3).sum::<f64>(), matched)
        })
        .collect::<Vec<_>>();
    let max_relevance = evidence
        .iter()
        .map(|(relevance, _)| *relevance)
        .fold(0.0_f64, f64::max);
    if max_relevance <= f64::EPSILON || !max_relevance.is_finite() {
        return hits;
    }
    let confidence_terms = query_terms.len().min(3) as f64;
    let mut reranked = hits
        .into_iter()
        .zip(evidence)
        .map(|((idx, score), (metadata, matched))| {
            // One rare exact title/tag match is already useful for a mixed-language
            // query (for example `WebSocket` plus Chinese reliability terms), but
            // two or three independent matches should still be stronger. A square-
            // root curve keeps that first signal material without letting repeated
            // metadata tokens dominate the lexical body score.
            let confidence =
                (matched.min(confidence_terms as usize) as f64).sqrt() / confidence_terms.sqrt();
            let metadata_contribution =
                strongest_lexical * MAX_METADATA_BOOST * confidence * metadata / max_relevance;
            let boosted = score + metadata_contribution;
            (idx, if boosted.is_finite() { boosted } else { score })
        })
        .collect::<Vec<_>>();
    reranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    reranked
}

/// Keep one highly-relevant document from consuming the entire prompt budget
/// through many adjacent H2 chunks. Learned incident files may contribute up
/// to two chunks; curated documents one. A second pass fills any spare
/// slots, so diversity never turns a sparse result set into an empty one.
fn diversify_paths(hits: Vec<ScoredChunk>, top_k: usize) -> Vec<ScoredChunk> {
    if top_k == 0 {
        return Vec::new();
    }
    let mut counts = HashMap::<String, usize>::new();
    let mut selected = Vec::with_capacity(top_k);
    let mut overflow = Vec::new();
    for hit in hits {
        let limit = if hit.chunk.meta.is_learned { 2 } else { 1 };
        let count = counts.entry(hit.chunk.meta.path.clone()).or_default();
        if *count < limit {
            *count += 1;
            selected.push(hit);
        } else {
            overflow.push(hit);
        }
    }
    if selected.len() < top_k {
        selected.extend(overflow.into_iter().take(top_k - selected.len()));
    }
    selected.truncate(top_k);
    selected
}

/// Collapse duplicate sedimented-lesson chunks so the SAME learned lesson is
/// never injected twice into one prompt.
///
/// The capture→sediment loop writes a lesson to BOTH the project dir
/// (`.umadev/learned/<domain>/lesson-*.md`) AND, once it's "global-worthy", the
/// user-home dir (`~/.umadev/learned/<domain>/<slug>.md`) with near-identical
/// content. Both dirs are indexed, so a plain BM25/RRF ranking can return the
/// project copy AND the global copy of one lesson — the worker then sees the
/// same guidance twice (noisy, and risks looking contradictory when an older
/// global copy lags a fresher project one).
///
/// This is the conservative half of the "unified reranker": rather than fuse the
/// agent crate's fingerprint channel in here (which would couple the two crates
/// and threaten the closed loop), we de-duplicate WITHIN this BM25/RRF channel
/// by CONTENT IDENTITY — `(section, title, first non-empty body line)`. Two
/// chunks sharing all three are the same material (the project + global copies
/// of one lesson have different filenames but identical content), so only the
/// first — i.e. higher-scored, since the list is already score-sorted — is kept.
///
/// Keying on content rather than a fragile `is_lesson` path heuristic means a
/// promoted-global lesson (whose filename is a slug, not `lesson-*`) still
/// collapses against its project copy. It is also safe for curated `knowledge/`:
/// distinct curated chunks never share an identical (section, title, first line)
/// triple, so they pass through untouched. Fail-open: order is otherwise
/// preserved and a unique chunk is never dropped.
fn dedup_learned_chunks(hits: Vec<ScoredChunk>) -> Vec<ScoredChunk> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<ScoredChunk> = Vec::with_capacity(hits.len());
    for hit in hits {
        // Upgrade quarantine: old auto-sediment pitfall markdown could contain
        // raw stderr/requirements, and generic `general/error/*` advice was too
        // coarse to act on. Filter it at the READ boundary so the very first
        // post-upgrade retrieval is safe even before a delivery re-sediments.
        if hit.chunk.meta.is_learned && is_quarantined_pitfall_chunk(&hit.chunk) {
            continue;
        }
        // Key on the FULL trimmed body, not just the first non-empty line. A prior
        // `(title, section, first_line)` key falsely collapsed two DISTINCT sub-chunks of
        // one oversized section that happen to share a first line — a ``` code fence, or a
        // boilerplate "This guide covers…" intro — silently dropping the second real chunk
        // from the results. Keying on the whole body means only a byte-identical duplicate
        // (a lesson sedimented twice) collapses; genuinely different content is always kept.
        let key = format!(
            "{}\0{}\0{}",
            hit.chunk.meta.title,
            hit.chunk.meta.section,
            hit.chunk.body.trim()
        );
        if seen.insert(key) {
            out.push(hit); // first (highest-scored) copy of this content
        }
        // else: a byte-identical, lower-scored copy → drop it.
    }
    out
}

fn is_quarantined_pitfall_chunk(chunk: &crate::chunker::Chunk) -> bool {
    let is_pitfall = chunk.meta.title.contains("[pitfall] Dev error")
        || chunk.body.contains("# [pitfall] Dev error:");
    if !is_pitfall {
        return false;
    }
    if chunk.meta.title.contains("general/error/") || chunk.body.contains("general/error/") {
        return true;
    }
    !chunk.meta.is_safe_learned_pitfall
}

/// Standard RRF constant. `k=60` is the value used by Elasticsearch and the
/// original Cormack et al. paper; it balances rank vs score contribution.
const RRF_K: u32 = 60;

/// Phase-aware retrieval — the most common entry point. Picks subdirs for
/// the phase, runs the query, returns chunks.
#[must_use]
pub fn retrieve_for_phase(
    project_root: &Path,
    knowledge_dir: &Path,
    config: &RetrievalConfig,
    query: &str,
    phase: Phase,
) -> Vec<ScoredChunk> {
    retrieve_with_vector(project_root, knowledge_dir, config, query, phase, None)
}

/// Phase-aware retrieval with a pre-embedded query vector (the hybrid path).
/// The async runner pre-embeds the requirement once, then calls this so
/// every phase gets true BM25+vector RRF fusion without re-embedding.
#[must_use]
pub fn retrieve_for_phase_with_vector(
    project_root: &Path,
    knowledge_dir: &Path,
    config: &RetrievalConfig,
    query: &str,
    phase: Phase,
    query_vec: Option<&[f32]>,
) -> Vec<ScoredChunk> {
    retrieve_with_vector(project_root, knowledge_dir, config, query, phase, query_vec)
}

/// Phase-aware retrieval with a pre-embedded query vector AND an optional
/// HyDE expansion. The single entry point the agent crate's coach seam uses
/// once it has generated a hypothetical answer: the expansion's BM25 ranking
/// is RRF-fused with the query's (see [`retrieve_with_vector_and_expansion`]),
/// composing on top of the existing BM25+vector fusion and the low-IDF mask.
/// `expansion = None` is identical to [`retrieve_for_phase_with_vector`].
#[must_use]
pub fn retrieve_for_phase_with_expansion(
    project_root: &Path,
    knowledge_dir: &Path,
    config: &RetrievalConfig,
    query: &str,
    phase: Phase,
    query_vec: Option<&[f32]>,
    expansion: Option<&str>,
) -> Vec<ScoredChunk> {
    retrieve_with_vector_and_expansion(
        project_root,
        knowledge_dir,
        config,
        query,
        phase,
        query_vec,
        expansion,
    )
}

/// Filter raw BM25 `(chunk_idx, score)` results to only chunks whose path
/// falls under a phase-relevant subdir, then take top_k.
fn filter_by_phase(
    index: &Bm25Index,
    raw: &[(usize, f64)],
    phase: Phase,
    top_k: usize,
) -> Vec<(usize, f64)> {
    // Phase subdirs: a per-phase OVERRIDE (UMADEV_KNOWLEDGE_PHASE_SUBDIRS)
    // replaces the static default when present; otherwise use the static map.
    // Either way, UMADEV_KNOWLEDGE_EXTRA_SUBDIRS is appended so a team can
    // both override specific phases AND add global extras.
    let extras: &[String] = extra_phase_subdirs();
    let base: Vec<&str> = match phase_subdirs_override(phase) {
        Some(override_dirs) => override_dirs.iter().map(String::as_str).collect(),
        None => phase_subdirs(phase).to_vec(),
    };
    let subdirs: Vec<&str> = base
        .into_iter()
        .chain(extras.iter().map(String::as_str))
        .collect();
    let subdirs: &[&str] = &subdirs;
    // Research scans the whole tree (empty subdirs = no filter).
    if subdirs.is_empty() || matches!(phase, Phase::Research) {
        return raw.iter().take(top_k).copied().collect();
    }
    let filtered: Vec<(usize, f64)> = raw
        .iter()
        .filter(|(idx, _)| {
            let path = &index.chunks[*idx].meta.path;
            // Always allow sedimented lessons through (they're cross-cutting
            // experience from prior runs). Lessons are pathed `<domain>/lesson-*`
            // after the .umadev/learned/ prefix is stripped, so we detect them by a
            // `lesson-` FILENAME PREFIX (not a loose substring: `path.contains("lesson-")`
            // also matched a curated `best-lesson-practices.md`, letting it bypass the
            // phase/subdir filter — #19).
            let file_name = path.rsplit(['/', '\\']).next().unwrap_or(path);
            let is_lesson = index.chunks[*idx].meta.is_learned || file_name.starts_with("lesson-");
            // Match on a full path SEGMENT, not a loose prefix: the subdir
            // `design` must match `design/x` but NOT `design-systems/x` (which
            // is inlined as the binding contract, not retrieved). Likewise
            // `mobile` must not match `mobile-foo`.
            let in_subdir = subdirs
                .iter()
                .any(|s| path == *s || path.starts_with(&format!("{s}/")));
            // Also accept the legacy `learned/` prefix (defensive).
            is_lesson || path.starts_with("learned") || in_subdir
        })
        .copied()
        .collect();
    // If phase-filtering wipes out everything, fall back to unfiltered top_k
    // so the prompt still gets context (better irrelevant than empty).
    if filtered.is_empty() && !raw.is_empty() {
        // Surface this through tracing, never stderr: retrieval can run while the
        // TUI owns the alternate screen, and direct stderr bytes corrupt frames.
        tracing::warn!(
            ?phase,
            ?subdirs,
            top_k,
            "knowledge phase-filter matched 0 chunks; falling back to unfiltered results"
        );
        raw.iter().take(top_k).copied().collect()
    } else {
        filtered.into_iter().take(top_k).collect()
    }
}

/// Normalise raw BM25/RRF scores to 0.0..=1.0 (best = 1.0) and attach chunks.
/// Extra knowledge subdirs to include in phase filtering, parsed from the
/// `UMADEV_KNOWLEDGE_EXTRA_SUBDIRS` env var (comma-separated). Cached for
/// the process lifetime. These are ADDED to every phase's static subdir list
/// so a custom knowledge/ layout can opt into filtering.
fn extra_phase_subdirs() -> &'static [String] {
    static CACHE: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        std::env::var("UMADEV_KNOWLEDGE_EXTRA_SUBDIRS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    })
}

/// Per-phase subdir OVERRIDE map parsed from
/// `UMADEV_KNOWLEDGE_PHASE_SUBDIRS`. Format: `phase:dir1,dir2;phase2:dir3`
/// (semicolon-separated `phase:dirs` entries; dirs comma-separated). When a
/// phase has an entry here, it FULLY REPLACES the static default subdirs for
/// that phase (the extras still apply). Lets a team whose knowledge/ layout
/// diverges from the built-in map override specific phases without forking.
/// Returns `Some(dirs)` when an override exists for `phase`, else `None`.
fn phase_subdirs_override(phase: Phase) -> Option<&'static [String]> {
    static CACHE: std::sync::OnceLock<std::collections::HashMap<String, Vec<String>>> =
        std::sync::OnceLock::new();
    let map = CACHE.get_or_init(|| {
        let raw = std::env::var("UMADEV_KNOWLEDGE_PHASE_SUBDIRS").unwrap_or_default();
        let mut m: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for entry in raw.split(';') {
            let entry = entry.trim();
            let Some((phase_part, dirs_part)) = entry.split_once(':') else {
                continue;
            };
            let dirs: Vec<String> = dirs_part
                .split(',')
                .map(|d| d.trim().to_string())
                .filter(|d| !d.is_empty())
                .collect();
            if !dirs.is_empty() {
                m.insert(phase_part.trim().to_ascii_lowercase(), dirs);
            }
        }
        m
    });
    map.get(phase.id()).map(Vec::as_slice)
}

/// The minimum normalised score (fraction of the top hit's score) a chunk
/// must reach to be kept. Default 0.5 — chunks scoring below 50% of the
/// best hit are treated as noise and dropped. Override with
/// `UMADEV_KNOWLEDGE_MIN_SCORE` (0.0 = keep everything, 1.0 = only exact
/// top-score ties). Useful for weak-match-heavy queries (CJK bigram queries
/// match many chunks loosely) where 0.5 drops everything.
fn min_score_filter() -> f32 {
    std::env::var("UMADEV_KNOWLEDGE_MIN_SCORE")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .map(|v| v.clamp(0.0, 1.0))
        .unwrap_or(0.5)
}

/// The IDF below which a query token is a candidate for low-IDF masking (the
/// absolute half of the test — see [`Bm25Index::mask_low_idf_terms`]). Default
/// `1.0`: with BM25's +1-smoothed IDF, a token appearing in roughly more than
/// ~40% of chunks falls under this, so only genuinely common terms qualify
/// (and they are still kept unless ALSO below the query's median IDF). Override
/// with `UMADEV_KNOWLEDGE_IDF_FLOOR` (e.g. `0.0` to effectively disable the
/// relative-IDF branch and mask on the stop list only).
fn idf_floor() -> f64 {
    std::env::var("UMADEV_KNOWLEDGE_IDF_FLOOR")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| v.max(0.0))
        .unwrap_or(1.0)
}

/// Applies a weak `quality_score` prior in the bounded range `0.95..=1.05`.
/// A missing score is 50 and therefore exactly neutral (`1.0`). Quality may
/// break a close relevance tie, but cannot rescue a substantially weaker hit.
///
/// Then blends the **usefulness prior** ([`crate::usefulness`]) at 10% strength:
/// the stored `0.3..=1.2` evidence becomes an effective `0.93..=1.02` weight.
/// Outcome memory may break a close relevance tie, but cannot overturn a
/// materially stronger current-query hit. Scores are normalised after sorting;
/// this
/// avoids saturating many strong hits at `1.0` and accidentally ordering those
/// ties by corpus path. `usefulness` is neutral (`1.0` for every chunk) on a
/// fresh corpus — BM25/vector relevance is blended, never replaced.
fn normalise(
    index: &Bm25Index,
    hits: Vec<(usize, f64)>,
    usefulness: &crate::usefulness::UsefulnessStore,
) -> Vec<ScoredChunk> {
    if hits.is_empty() {
        return Vec::new();
    }
    let raw_max = hits
        .iter()
        .map(|(_, score)| *score)
        .filter(|score| score.is_finite() && *score > 0.0)
        .fold(0.0_f64, f64::max)
        .max(1e-9);
    let min_score = min_score_filter();
    let mut weighted: Vec<(usize, f32)> = hits
        .into_iter()
        .filter_map(|(idx, score)| {
            let chunk = index.chunks.get(idx)?;
            if !score.is_finite() || score <= 0.0 {
                return None;
            }
            let base = (score / raw_max) as f32;
            let qs = chunk.quality_score.unwrap_or(50).clamp(0, 100);
            // Usefulness prior: 1.0 until this exact chunk body is well-sampled,
            // then 0.3..=1.2 by its helpful ratio. Content identity prevents two
            // colliding headings or a later edit from inheriting unrelated trust.
            let meta = &chunk.meta;
            let memory =
                crate::usefulness::MemoryRef::from_parts(&meta.path, &meta.section, &chunk.body);
            let usefulness_w = usefulness.weight_for_memory(&memory);
            let usefulness_w = 1.0 + (usefulness_w - 1.0) * USEFULNESS_BLEND;
            let quality_w = 0.95 + qs as f32 / 1_000.0;
            let origin_w = corpus_origin_weight(chunk.meta.corpus_origin);
            let boosted = base * quality_w * usefulness_w * origin_w;
            boosted.is_finite().then_some((idx, boosted))
        })
        .collect();
    if weighted.is_empty() {
        return Vec::new();
    }
    weighted.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    let boosted_max = weighted[0].1.max(f32::EPSILON);
    weighted
        .into_iter()
        .filter_map(|(idx, score)| {
            let score = (score / boosted_max).clamp(0.0, 1.0);
            (score >= min_score).then(|| ScoredChunk {
                chunk: index.chunks[idx].clone(),
                score,
            })
        })
        .collect()
}

/// Fuse the CJK trigram channel into a bigram-channel ranking.
///
/// The trigram channel is a parallel BM25 ranking over the query's 3-char-CJK
/// windows, matched against the trigram tokens the chunker appended to each
/// chunk. Trigrams carry one more character of local context than bigrams, so
/// substring / short-phrase CJK matches land more precisely. The two rankings
/// are RRF-fused (the same rank fusion HyDE uses).
///
/// Fail-open / identity-preserving: a query with no 3-char-CJK window yields no
/// trigram terms, and a trigram search that matches nothing both return the
/// bigram ranking unchanged — so non-CJK and short-CJK queries are byte-for-byte
/// the prior behaviour.
fn fuse_trigram_channel(
    index: &Bm25Index,
    query: &str,
    bigram: Vec<(usize, f64)>,
    over_fetch: usize,
) -> Vec<(usize, f64)> {
    // Gate on a genuine CJK trigram: an ASCII-only (or short-CJK) query carries
    // no 3-char-CJK window, so fusing would only re-rank on the SAME ASCII terms
    // the bigram channel already used — a no-op for order but it would perturb
    // scores. Keeping the gate makes non-CJK retrieval byte-for-byte unchanged.
    let trigram_terms = crate::tokenizer::cjk_trigrams_only(query);
    if trigram_terms.is_empty() {
        return bigram;
    }
    // Search the FULL trigram view (CJK windows + any ASCII identifiers in the
    // query) so a mixed-script phrase contributes both channels' signal.
    let trigram = index.search_terms(&crate::tokenizer::tokenize_trigram(query), over_fetch);
    if trigram.is_empty() {
        return bigram;
    }
    rrf_fuse_bm25(&bigram, &trigram, RRF_K, over_fetch)
}

/// Reciprocal Rank Fusion of TWO BM25 ranked lists that share the same
/// address space (both key on the index's `chunk_idx`). Used to fuse the
/// original query's ranking with a HyDE expansion's ranking: a chunk surfaced
/// by EITHER list scores `1/(k+rank)` from that list, and a chunk surfaced by
/// BOTH (the strongest signal — query AND hypothetical agree) sums the two.
///
/// Simpler than [`rrf_fuse`] (the BM25↔vector fuser) because no `(path,
/// section) → idx` remapping is needed — both inputs already speak chunk
/// indices. Returns chunk indices ranked by fused score, truncated to `top_k`.
fn rrf_fuse_bm25(
    primary: &[(usize, f64)],
    secondary: &[(usize, f64)],
    k: u32,
    top_k: usize,
) -> Vec<(usize, f64)> {
    let kf = f64::from(k);
    let mut scores: HashMap<usize, f64> = HashMap::new();
    for (rank, (idx, _)) in primary.iter().enumerate() {
        *scores.entry(*idx).or_insert(0.0) += 1.0 / (kf + rank as f64 + 1.0);
    }
    for (rank, (idx, _)) in secondary.iter().enumerate() {
        *scores.entry(*idx).or_insert(0.0) += 1.0 / (kf + rank as f64 + 1.0);
    }
    let mut fused: Vec<(usize, f64)> = scores.into_iter().collect();
    // Deterministic tiebreak: equal fused scores are common (two chunks at the
    // same rank in each list), and collecting from a HashMap yields them in
    // arbitrary iteration order. Break ties by ascending chunk index so the
    // fused ranking is reproducible run-to-run (the crate's stated determinism).
    fused.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    fused.truncate(top_k);
    fused
}

/// Reciprocal Rank Fusion — merge BM25 and vector ranked lists by
/// `1/(k + rank)`. `k=60` is the standard RRF constant (Elasticsearch,
/// original Cormack et al. paper).
///
/// Both lists now address chunks by the SAME positional `chunk_idx`: BM25
/// natively, and vector hits via [`VectorStore::search_with_idx`] (the store's
/// per-chunk `chunk_idx` is built over the same multi-dir index retrieval reads).
/// A chunk appearing in both lists gets a higher fused score (the whole point of
/// hybrid retrieval). Returns chunk indices ranked by fused score, truncated to
/// `top_k`.
///
/// P0-2: the previous implementation re-mapped vector hits through a `(path,
/// section) → chunk_idx` table and DROPPED any colliding key — but `(path,
/// section)` collisions are the NORM (synthetic `Overview`/`Document` sections;
/// the `knowledge/` vs `learned/` path overlap), so legitimate, distinctly
/// indexed vector hits were silently lost (a retrieval leak). Keying directly on
/// the collision-safe `chunk_idx` ends that — a vector hit can never overwrite or
/// be dropped on behalf of a different chunk that merely shares a section name.
/// A stale `chunk_idx` (source chunk removed since the store was built) is simply
/// ignored when it falls outside the current index — fail-open, never a panic.
fn rrf_fuse(
    index: &Bm25Index,
    bm25: &[(usize, f64)],
    vector_hits: &[(u32, f32)],
    k: u32,
    top_k: usize,
) -> Vec<(usize, f64)> {
    let n_chunks = index.chunks.len();
    let mut scores: HashMap<usize, f64> = HashMap::new();
    let kf = f64::from(k);

    // BM25 contribution: rank 0 is the top hit.
    for (rank, (chunk_idx, _)) in bm25.iter().enumerate() {
        *scores.entry(*chunk_idx).or_insert(0.0) += 1.0 / (kf + rank as f64 + 1.0);
    }

    // Vector contribution: rank 0 is the top hit. Key DIRECTLY on chunk_idx (no
    // lossy `(path, section)` remap). Drop only a genuinely STALE index — one
    // that points past the current chunk set (its source chunk was removed since
    // the store was built) — never a valid hit that merely collides on section.
    for (rank, (chunk_idx, _)) in vector_hits.iter().enumerate() {
        let idx = *chunk_idx as usize;
        if idx >= n_chunks {
            continue; // stale vector — source chunk no longer in the index
        }
        *scores.entry(idx).or_insert(0.0) += 1.0 / (kf + rank as f64 + 1.0);
    }

    let mut fused: Vec<(usize, f64)> = scores.into_iter().collect();
    // Deterministic tiebreak on ascending chunk index — see [`rrf_fuse_bm25`].
    // Without it, equal-scored chunks come out of the HashMap in arbitrary order
    // and the final ranking (and what `top_k` keeps) varies run-to-run.
    fused.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    fused.truncate(top_k);
    fused
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::load_or_build_index_multi;
    use std::fs;
    use std::path::PathBuf;

    fn seed_corpus(root: &Path) -> PathBuf {
        let kd = root.join("knowledge");
        fs::create_dir_all(kd.join("security")).unwrap();
        fs::write(
            kd.join("security/login.md"),
            "# Login Playbook\n\n## OAuth\n\nUse OAuth2 with PKCE for login authentication.\n\n## Risks\n\nToken theft.",
        )
        .unwrap();
        fs::create_dir_all(kd.join("database")).unwrap();
        fs::write(
            kd.join("database/postgres.md"),
            "# Postgres\n\n## Tuning\n\nshared_buffers and work_mem tuning for the database.",
        )
        .unwrap();
        kd
    }

    #[test]
    fn retrieve_returns_relevant_chunk() {
        let tmp = tempfile::TempDir::new().unwrap();
        let kd = seed_corpus(tmp.path());
        let cfg = RetrievalConfig::default();
        let hits = retrieve(tmp.path(), &kd, &cfg, "login oauth", Phase::Research);
        assert!(!hits.is_empty());
        assert!(hits[0].chunk.meta.path.contains("login"));
        assert!(hits[0].score > 0.0);
    }

    #[test]
    fn cjk_query_retrieves_relevant_content() {
        let tmp = tempfile::TempDir::new().unwrap();
        let kd = tmp.path().join("knowledge");
        fs::create_dir_all(kd.join("security")).unwrap();
        fs::write(
            kd.join("security/login.md"),
            "# 登录系统\n\n## 流程\n\n使用 OAuth2 做登录认证",
        )
        .unwrap();
        let cfg = RetrievalConfig::default();
        let hits = retrieve(tmp.path(), &kd, &cfg, "做一个登录系统", Phase::Research);
        assert!(
            !hits.is_empty(),
            "CJK requirement must retrieve CJK content"
        );
    }

    #[test]
    fn bilingual_alias_channel_recalls_an_english_only_document_from_chinese() {
        let tmp = tempfile::TempDir::new().unwrap();
        let kd = tmp.path().join("knowledge");
        fs::create_dir_all(kd.join("security")).unwrap();
        fs::write(
            kd.join("security/credential-rotation.md"),
            "# Credential lifecycle\n\n## Rotation\n\nRotate authentication credentials on a bounded schedule.",
        )
        .unwrap();
        let cfg = RetrievalConfig {
            engine: RetrievalEngine::Bm25,
            ..RetrievalConfig::default()
        };
        let hits = retrieve(tmp.path(), &kd, &cfg, "登录凭证轮换", Phase::Research);
        assert!(
            hits.iter()
                .any(|hit| hit.chunk.meta.path.contains("credential-rotation")),
            "Chinese synonyms must recall the English-only source: {hits:?}"
        );
    }

    #[test]
    fn bilingual_alias_channel_recalls_a_chinese_only_document_from_english() {
        let tmp = tempfile::TempDir::new().unwrap();
        let kd = tmp.path().join("knowledge");
        fs::create_dir_all(kd.join("backend")).unwrap();
        fs::write(
            kd.join("backend/payment.md"),
            "# 支付可靠性\n\n## 请求处理\n\n支付接口必须使用幂等键，超时后允许安全重试。",
        )
        .unwrap();
        let cfg = RetrievalConfig {
            engine: RetrievalEngine::Bm25,
            ..RetrievalConfig::default()
        };
        let hits = retrieve(
            tmp.path(),
            &kd,
            &cfg,
            "idempotent retry for payment",
            Phase::Research,
        );
        assert!(
            hits.iter()
                .any(|hit| hit.chunk.meta.path.contains("payment")),
            "English synonyms must recall the Chinese-only source: {hits:?}"
        );
    }

    #[test]
    fn literal_query_evidence_outranks_alias_only_evidence() {
        let tmp = tempfile::TempDir::new().unwrap();
        let kd = tmp.path().join("knowledge");
        fs::create_dir_all(kd.join("security")).unwrap();
        fs::write(
            kd.join("security/literal.md"),
            "# 登录认证\n\n## 登录认证\n\n登录认证必须校验当前用户身份。",
        )
        .unwrap();
        fs::write(
            kd.join("security/alias-only.md"),
            "# Authentication\n\n## Authentication\n\nAuthentication verifies the current user identity.",
        )
        .unwrap();
        let cfg = RetrievalConfig {
            engine: RetrievalEngine::Bm25,
            ..RetrievalConfig::default()
        };
        let hits = retrieve(tmp.path(), &kd, &cfg, "登录认证", Phase::Research);
        assert_eq!(
            hits.first().map(|hit| hit.chunk.meta.path.as_str()),
            Some("security/literal.md"),
            "the untouched-query channel must outrank alias-only inference: {hits:?}"
        );
    }

    #[test]
    fn unrelated_query_abstains_even_with_bilingual_expansion_enabled() {
        let tmp = tempfile::TempDir::new().unwrap();
        let kd = seed_corpus(tmp.path());
        let cfg = RetrievalConfig {
            engine: RetrievalEngine::Bm25,
            ..RetrievalConfig::default()
        };
        assert!(retrieve(
            tmp.path(),
            &kd,
            &cfg,
            "quasar pottery musical notation",
            Phase::Research,
        )
        .is_empty());
    }

    #[test]
    fn disabled_config_returns_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let kd = seed_corpus(tmp.path());
        let cfg = RetrievalConfig {
            enabled: false,
            ..RetrievalConfig::default()
        };
        assert!(corpus_set(tmp.path(), &kd, &cfg).is_empty());
        assert!(retrieve(tmp.path(), &kd, &cfg, "login", Phase::Research).is_empty());
        let explicit = CorpusSet::from_roots([(
            kd,
            crate::CorpusOrigin::ProjectCustom,
            crate::CorpusScope::Project,
        )]);
        assert!(retrieve_corpus(tmp.path(), &explicit, &cfg, "login", Phase::Research).is_empty());
    }

    #[test]
    fn empty_query_returns_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let kd = seed_corpus(tmp.path());
        let cfg = RetrievalConfig::default();
        assert!(retrieve(tmp.path(), &kd, &cfg, "   ", Phase::Research).is_empty());
    }

    #[test]
    fn phase_filter_narrows_to_subdirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let kd = seed_corpus(tmp.path());
        let cfg = RetrievalConfig::default();
        // Quality phase maps to testing/security/00-governance — should still
        // find the security/login.md doc.
        let hits = retrieve(tmp.path(), &kd, &cfg, "login", Phase::Quality);
        assert!(hits.iter().any(|h| h.chunk.meta.path.contains("security")));
    }

    #[test]
    fn phase_filter_falls_back_when_no_match() {
        let tmp = tempfile::TempDir::new().unwrap();
        let kd = seed_corpus(tmp.path());
        let cfg = RetrievalConfig::default();
        // Backend phase subdirs are backend/api/database/security — a query
        // matching only the postgres doc should still hit.
        let hits = retrieve(
            tmp.path(),
            &kd,
            &cfg,
            "postgres database tuning",
            Phase::Backend,
        );
        assert!(!hits.is_empty());
    }

    #[test]
    fn top_k_limits_results() {
        let tmp = tempfile::TempDir::new().unwrap();
        let kd = seed_corpus(tmp.path());
        let cfg = RetrievalConfig {
            top_k: 1,
            ..RetrievalConfig::default()
        };
        let hits = retrieve(tmp.path(), &kd, &cfg, "auth login", Phase::Research);
        assert!(hits.len() <= 1);
    }

    #[test]
    fn scores_normalised_to_top_is_one() {
        let tmp = tempfile::TempDir::new().unwrap();
        let kd = seed_corpus(tmp.path());
        let cfg = RetrievalConfig::default();
        let hits = retrieve(tmp.path(), &kd, &cfg, "login", Phase::Research);
        if !hits.is_empty() {
            assert!((hits[0].score - 1.0).abs() < 1e-5, "top hit should be ~1.0");
        }
    }

    #[test]
    fn phase_subdirs_known_phases() {
        assert!(!phase_subdirs(Phase::Backend).is_empty());
        assert!(!phase_subdirs(Phase::Frontend).is_empty());
        assert!(phase_subdirs(Phase::Research).is_empty()); // whole-tree scan
        assert!(phase_subdirs(Phase::DocsConfirm).is_empty()); // gate, no retrieval
    }

    #[test]
    fn config_round_trips() {
        let cfg = RetrievalConfig {
            enabled: false,
            engine: RetrievalEngine::Hybrid,
            top_k: 12,
            custom_dirs: vec!["team/".into()],
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: RetrievalConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.engine, RetrievalEngine::Hybrid);
        assert_eq!(back.top_k, 12);
        assert!(!back.enabled);
    }

    #[test]
    fn metadata_fields_have_bounded_precedence() {
        let chunk = crate::chunker::chunk_text(
            "release-engineering/03-checklists/readiness-checklist.md",
            "---\ntags: [rollback]\n---\n# Canary Guide\n\n## Failover Steps\n\nbody",
        )
        .remove(0);
        let fields = MetadataTokens::from_chunk(&chunk);
        for (term, expected) in [
            ("canary", 3.0),
            ("rollback", 2.5),
            ("failover", 1.75),
            ("release", 1.25),
            ("readiness", 1.0),
            ("unmatched", 0.0),
        ] {
            assert!((fields.term_weight(term) - expected).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn metadata_rerank_promotes_specific_hit_without_overriding_weak_hit() {
        let body_only = crate::chunker::chunk_text(
            "misc/other.md",
            "# Other\n\n## Notes\n\nrelease rollback recovery details",
        )
        .remove(0);
        let specific = crate::chunker::chunk_text(
            "release-engineering/release-rollback.md",
            "---\ntags: [recovery]\n---\n# Release Rollback\n\n## Guide\n\nbrief",
        )
        .remove(0);
        let weak = crate::chunker::chunk_text(
            "release-engineering/weak.md",
            "# Release Rollback\n\n## Guide\n\nbrief",
        )
        .remove(0);
        let index = Bm25Index::from_chunks(vec![body_only, specific, weak]);
        let input = vec![(0, 1.0), (1, 0.8), (2, 0.4)];
        let expected = vec![1, 0, 2];
        for _ in 0..16 {
            let ranked = rerank_by_metadata(&index, "release rollback recovery", input.clone());
            assert_eq!(
                ranked.iter().map(|(idx, _)| *idx).collect::<Vec<_>>(),
                expected
            );
        }
    }

    #[test]
    fn metadata_rerank_fails_open_on_invalid_input() {
        let chunk = crate::chunker::chunk_text("a.md", "# A\n\n## S\n\nbody").remove(0);
        let index = Bm25Index::from_chunks(vec![chunk]);
        let invalid_index = vec![(4, 1.0), (0, 0.8)];
        assert_eq!(
            rerank_by_metadata(&index, "body", invalid_index.clone()),
            invalid_index
        );
        let non_finite = vec![(0, f64::NAN), (0, 0.8)];
        let result = rerank_by_metadata(&index, "body", non_finite.clone());
        assert!(result[0].1.is_nan());
        assert_eq!(result[1], non_finite[1]);
    }

    #[test]
    fn curated_metadata_channel_keeps_exact_language_document_candidates() {
        let project = tempfile::tempdir().unwrap();
        let knowledge = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("knowledge");
        let index = load_or_build_index_multi(project.path(), &[knowledge]);
        for (query, expected_path) in [
            (
                "Rust borrow checker ownership lifetime mutable reference Send Sync 并发安全",
                "development/01-standards/rust-complete.md",
            ),
            (
                "跨服务事务不用 2PC，比较 saga transactional outbox 补偿和幂等消费者",
                "architecture/distributed-transactions.md",
            ),
        ] {
            let lexical = index.search(query, 30);
            let metadata = metadata_rank(&index, query, &lexical, 30);
            let fused = rrf_fuse_bm25(&lexical, &metadata, RRF_K, 30);
            let reranked = rerank_by_metadata(&index, query, fused);
            let final_hits = diversify_paths(
                dedup_learned_chunks(normalise(
                    &index,
                    reranked,
                    &crate::usefulness::UsefulnessStore::default(),
                )),
                10,
            );
            assert!(
                metadata
                    .iter()
                    .any(|(idx, _)| index.chunks[*idx].meta.path == expected_path),
                "metadata candidate generation dropped {expected_path}"
            );
            let final_rank = final_hits
                .iter()
                .position(|hit| hit.chunk.meta.path == expected_path);
            assert!(
                final_rank.is_some_and(|rank| rank < 5),
                "{expected_path} must survive metadata fusion in the top five; got {final_rank:?}"
            );
        }
    }

    #[test]
    fn path_diversification_samples_documents_before_overflow() {
        let mut same = crate::chunker::chunk_text(
            "guide/a.md",
            "# A\n\n## One\n\none\n\n## Two\n\ntwo\n\n## Three\n\nthree",
        );
        let other = crate::chunker::chunk_text("guide/b.md", "# B\n\n## One\n\nbody").remove(0);
        let hits = vec![
            ScoredChunk {
                chunk: same.remove(0),
                score: 1.0,
            },
            ScoredChunk {
                chunk: same.remove(0),
                score: 0.9,
            },
            ScoredChunk {
                chunk: other,
                score: 0.8,
            },
        ];
        let diverse = diversify_paths(hits.clone(), 2);
        assert_eq!(
            diverse
                .iter()
                .map(|hit| hit.chunk.meta.path.as_str())
                .collect::<Vec<_>>(),
            ["guide/a.md", "guide/b.md"]
        );
        assert_eq!(diversify_paths(hits, 3).len(), 3);
    }

    #[test]
    fn learned_path_diversification_allows_two_incident_sections() {
        let mut learned = crate::chunker::chunk_text(
            "dependency/lesson.md",
            "# Lesson\n\n## One\n\none\n\n## Two\n\ntwo\n\n## Three\n\nthree",
        );
        for chunk in &mut learned {
            chunk.meta.is_learned = true;
        }
        let other =
            crate::chunker::chunk_text("dependency/other.md", "# B\n\n## One\n\nbody").remove(0);
        let mut hits = learned
            .into_iter()
            .enumerate()
            .map(|(rank, chunk)| ScoredChunk {
                chunk,
                score: 1.0 - rank as f32 / 10.0,
            })
            .collect::<Vec<_>>();
        hits.push(ScoredChunk {
            chunk: other,
            score: 0.6,
        });
        let diverse = diversify_paths(hits, 3);
        assert_eq!(
            diverse
                .iter()
                .filter(|hit| hit.chunk.meta.path == "dependency/lesson.md")
                .count(),
            2
        );
        assert!(diverse
            .iter()
            .any(|hit| hit.chunk.meta.path == "dependency/other.md"));
    }

    #[test]
    fn dedup_learned_chunks_collapses_duplicate_lessons() {
        // Two copies of ONE sedimented lesson (project + global) with different
        // paths but identical title + body → must collapse to a single hit.
        let proj = crate::chunker::chunk_text(
            "frontend/lesson-frontend-1.md",
            "# Lesson\n\n## Symptom\n\nthe avoid-color pitfall body here",
        );
        let glob = crate::chunker::chunk_text(
            "frontend/api-Validated-color.md",
            "# Lesson\n\n## Symptom\n\nthe avoid-color pitfall body here",
        );
        // A distinct curated knowledge chunk that must NOT be touched.
        let curated = crate::chunker::chunk_text(
            "design/tokens.md",
            "# Tokens\n\n## Color\n\nuse design tokens not hex",
        );
        let mk = |c: &Chunk, s: f32| ScoredChunk {
            chunk: c.clone(),
            score: s,
        };
        let hits = vec![
            mk(&proj[0], 1.0),    // higher-scored copy → kept
            mk(&glob[0], 0.8),    // duplicate lesson → dropped
            mk(&curated[0], 0.7), // distinct knowledge → kept
        ];
        let out = dedup_learned_chunks(hits);
        assert_eq!(out.len(), 2, "duplicate lesson collapsed, curated kept");
        // The kept lesson copy is the higher-scored project one.
        assert!(out[0].chunk.meta.path.contains("lesson-frontend-1"));
        assert!(out
            .iter()
            .any(|h| h.chunk.meta.path.contains("design/tokens")));
    }

    #[test]
    fn learned_pitfall_read_boundary_quarantines_generic_and_legacy_unsafe_chunks() {
        let make = |title: &str, body: &str, is_safe: bool| {
            let mut chunk = crate::chunker::chunk_text(
                "dependency/legacy-pitfall.md",
                &format!("# {title}\n\n## Symptom\n\n{body}"),
            )
            .remove(0);
            chunk.meta.is_learned = true;
            chunk.meta.is_safe_learned_pitfall = is_safe;
            ScoredChunk { chunk, score: 1.0 }
        };
        let generic = make(
            "[pitfall] Dev error: generic",
            "signature general/error/failed and copied stderr",
            false,
        );
        let unsafe_legacy = make(
            "[pitfall] Dev error: module",
            "signature dependency/module-not-found/lodash raw /Users/private",
            false,
        );
        let prose_spoof = make(
            "[pitfall] Dev error: module",
            "copied text says Raw stderr and requirement text are intentionally excluded",
            false,
        );
        let safe = make(
            "[pitfall] Dev error: module",
            "signature dependency/module-not-found/lodash",
            true,
        );
        let out = dedup_learned_chunks(vec![generic, unsafe_legacy, prose_spoof, safe]);
        assert_eq!(out.len(), 1);
        assert!(out[0].chunk.meta.is_safe_learned_pitfall);
    }

    #[test]
    fn dedup_keeps_distinct_lessons() {
        // Two genuinely different lessons (different titles) must both survive.
        let a =
            crate::chunker::chunk_text("frontend/lesson-frontend-1.md", "# A\n\n## S\n\nbody a");
        let b =
            crate::chunker::chunk_text("frontend/lesson-frontend-2.md", "# B\n\n## S\n\nbody b");
        let mk = |c: &Chunk, s: f32| ScoredChunk {
            chunk: c.clone(),
            score: s,
        };
        let out = dedup_learned_chunks(vec![mk(&a[0], 1.0), mk(&b[0], 0.9)]);
        assert_eq!(out.len(), 2, "distinct lessons must not be merged");
    }

    #[test]
    fn dedup_keeps_distinct_chunks_that_share_a_first_line() {
        // #11 — two DISTINCT sub-chunks of one oversized section share title + section AND
        // their first non-empty line (a ``` code fence), but their bodies differ. The old
        // (title, section, first_line) key falsely collapsed them, dropping the second real
        // chunk; keying on the full body must keep BOTH.
        let base = &crate::chunker::chunk_text("kb/guide.md", "# Guide\n\n## Setup\n\nseed")[0];
        let mut a = base.clone();
        a.body = "```\nstep one: install the dependencies\n```".to_string();
        let mut b = base.clone();
        b.body = "```\nstep two: configure the environment\n```".to_string();
        assert_eq!(a.meta.title, b.meta.title);
        assert_eq!(a.meta.section, b.meta.section);
        let mk = |c: &Chunk, s: f32| ScoredChunk {
            chunk: c.clone(),
            score: s,
        };
        let out = dedup_learned_chunks(vec![mk(&a, 1.0), mk(&b, 0.9)]);
        assert_eq!(
            out.len(),
            2,
            "distinct chunks sharing a first line must both survive"
        );
    }

    #[test]
    fn rrf_fuse_merges_and_promotes_overlap() {
        let chunks = crate::chunker::chunk_text(
            "security/login.md",
            "# Login\n\n## OAuth\n\noauth pkce\n\n## Risks\ntoken theft",
        );
        let index = Bm25Index::from_chunks(chunks);
        // BM25 ranks chunk 0 (OAuth) first, chunk 1 (Risks) second.
        let bm25: Vec<(usize, f64)> = vec![(0, 5.0), (1, 1.0)];
        // Vector (keyed on chunk_idx) also ranks chunk 0 first.
        let vec_hits: Vec<(u32, f32)> = vec![(0, 0.98), (1, 0.70)];
        let fused = rrf_fuse(&index, &bm25, &vec_hits, 60, 5);
        // Chunk 0 appears at rank 0 in both lists → highest fused score.
        assert_eq!(fused[0].0, 0);
        assert!(fused[0].1 > fused[1].1, "overlap chunk must outrank solo");
    }

    #[test]
    fn rrf_fuse_drops_stale_vector_idx() {
        let chunks = crate::chunker::chunk_text("a.md", "# A\n\n## S\n\nbody");
        let index = Bm25Index::from_chunks(chunks); // 1 chunk → valid idx is 0
        let bm25: Vec<(usize, f64)> = vec![(0, 3.0)];
        // A vector hit pointing past the current chunk set (its source chunk was
        // removed since the store was built) is ignored — fail-open, no panic.
        let vec_hits: Vec<(u32, f32)> = vec![(99, 0.9)];
        let fused = rrf_fuse(&index, &bm25, &vec_hits, 60, 5);
        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].0, 0);
    }

    #[test]
    fn rrf_fuse_keeps_colliding_section_chunks_distinct() {
        // P0-2 regression: two DIFFERENT chunks that share the same (path,
        // section) — the norm for synthetic `Document`/`Overview` sections and the
        // knowledge/ vs learned/ path overlap — must BOTH be fusable. The old
        // (path, section) remap dropped one of them; keying on chunk_idx keeps
        // them distinct. Build an index whose chunks 0 and 1 share (path, section).
        let mut a = crate::chunker::chunk_text("security/x.md", "# X\n\nbody-a")[0].clone();
        let mut b = a.clone();
        // Force identical (path, section) on two distinct chunks.
        a.meta.section = "Document".to_string();
        b.meta.section = "Document".to_string();
        a.body = "alpha".to_string();
        b.body = "beta".to_string();
        let index = Bm25Index::from_chunks(vec![a, b]);
        // BM25 surfaced only chunk 0; the VECTOR channel surfaced chunk 1 (the
        // colliding sibling). Both must contribute — chunk 1 must NOT be dropped.
        let bm25: Vec<(usize, f64)> = vec![(0, 4.0)];
        let vec_hits: Vec<(u32, f32)> = vec![(1, 0.95)];
        let fused = rrf_fuse(&index, &bm25, &vec_hits, 60, 5);
        let ids: Vec<usize> = fused.iter().map(|(i, _)| *i).collect();
        assert!(
            ids.contains(&0) && ids.contains(&1),
            "both colliding-section chunks must survive fusion: {ids:?}"
        );
    }

    #[test]
    fn rrf_fuse_bm25_tiebreak_is_deterministic_by_index() {
        // Two non-overlapping chunks, each at rank 0 in its own list, tie on
        // fused score. The tiebreak must order them by ASCENDING chunk index,
        // reproducibly — not the arbitrary HashMap iteration order.
        let primary: Vec<(usize, f64)> = vec![(7, 1.0)];
        let secondary: Vec<(usize, f64)> = vec![(3, 1.0)];
        for _ in 0..64 {
            let fused = rrf_fuse_bm25(&primary, &secondary, 60, 5);
            assert_eq!(fused.len(), 2);
            assert!(
                (fused[0].1 - fused[1].1).abs() < 1e-12,
                "the two solo hits must tie on score"
            );
            assert_eq!(fused[0].0, 3, "lower chunk index wins the tie");
            assert_eq!(fused[1].0, 7);
        }
    }

    #[test]
    fn rrf_fuse_tiebreak_is_deterministic_by_index() {
        // Same determinism guarantee for the BM25 vector fuser: two chunks that
        // tie on fused score come out ascending-index ordered every run.
        let chunks = crate::chunker::chunk_text(
            "a.md",
            "# A\n\n## One\n\nalpha\n\n## Two\n\nbeta\n\n## Three\n\ngamma",
        );
        let index = Bm25Index::from_chunks(chunks);
        assert!(index.chunks.len() >= 2);
        // Chunk 1 from BM25 (rank 0), chunk 0 from vector (rank 0) → equal score.
        let bm25: Vec<(usize, f64)> = vec![(1, 4.0)];
        let vec_hits: Vec<(u32, f32)> = vec![(0, 0.9)];
        for _ in 0..64 {
            let fused = rrf_fuse(&index, &bm25, &vec_hits, 60, 5);
            assert_eq!(fused.len(), 2);
            assert!((fused[0].1 - fused[1].1).abs() < 1e-12, "scores tie");
            assert_eq!(fused[0].0, 0, "lower chunk index wins the tie");
            assert_eq!(fused[1].0, 1);
        }
    }

    #[test]
    fn rrf_fuse_bm25_sums_overlap_and_unions() {
        // chunk 0 is top of BOTH lists → highest fused score; chunk 2 appears in
        // only the secondary list → still included (union), but lower.
        let primary: Vec<(usize, f64)> = vec![(0, 5.0), (1, 2.0)];
        let secondary: Vec<(usize, f64)> = vec![(0, 4.0), (2, 1.0)];
        let fused = rrf_fuse_bm25(&primary, &secondary, 60, 5);
        assert_eq!(fused[0].0, 0, "the chunk in both lists must lead");
        let ids: Vec<usize> = fused.iter().map(|(i, _)| *i).collect();
        assert!(ids.contains(&1) && ids.contains(&2), "union of both lists");
        // chunk 0 (in both) outscores chunk 1 and chunk 2 (each in one).
        assert!(fused[0].1 > fused[1].1);
    }

    #[test]
    fn expansion_none_equals_plain_retrieval() {
        // The HyDE entry point with expansion=None must be byte-for-byte the
        // prior behaviour.
        let tmp = tempfile::TempDir::new().unwrap();
        let kd = seed_corpus(tmp.path());
        let cfg = RetrievalConfig::default();
        let plain = retrieve(tmp.path(), &kd, &cfg, "login oauth", Phase::Research);
        let with_none = retrieve_with_vector_and_expansion(
            tmp.path(),
            &kd,
            &cfg,
            "login oauth",
            Phase::Research,
            None,
            None,
        );
        let paths_a: Vec<_> = plain.iter().map(|h| h.chunk.meta.path.clone()).collect();
        let paths_b: Vec<_> = with_none
            .iter()
            .map(|h| h.chunk.meta.path.clone())
            .collect();
        assert_eq!(paths_a, paths_b, "expansion=None must not change results");
    }

    #[test]
    fn expansion_recalls_a_doc_the_query_misses() {
        // The query shares NO tokens with the postgres doc; the HyDE expansion
        // (answer vocabulary) does. Fusing must surface postgres that the bare
        // query alone would never reach.
        let tmp = tempfile::TempDir::new().unwrap();
        let kd = seed_corpus(tmp.path());
        let cfg = RetrievalConfig::default();
        // Bare query shares no answer-vocabulary tokens with the postgres doc.
        let bare = retrieve(tmp.path(), &kd, &cfg, "quasar request", Phase::Research);
        assert!(
            !bare.iter().any(|h| h.chunk.meta.path.contains("postgres")),
            "bare query should not reach the postgres doc"
        );
        // With a hypothetical answer mentioning the DB-tuning vocabulary.
        let fused = retrieve_with_vector_and_expansion(
            tmp.path(),
            &kd,
            &cfg,
            "quasar request",
            Phase::Research,
            None,
            Some("Use shared_buffers and work_mem tuning for the database."),
        );
        assert!(
            fused.iter().any(|h| h.chunk.meta.path.contains("postgres")),
            "HyDE expansion must recall the doc the bare query missed: {:?}",
            fused
                .iter()
                .map(|hit| (&hit.chunk.meta.path, hit.score))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn empty_expansion_is_identity() {
        let tmp = tempfile::TempDir::new().unwrap();
        let kd = seed_corpus(tmp.path());
        let cfg = RetrievalConfig::default();
        let none = retrieve_with_vector_and_expansion(
            tmp.path(),
            &kd,
            &cfg,
            "login",
            Phase::Research,
            None,
            None,
        );
        let blank = retrieve_with_vector_and_expansion(
            tmp.path(),
            &kd,
            &cfg,
            "login",
            Phase::Research,
            None,
            Some("   "),
        );
        let a: Vec<_> = none.iter().map(|h| h.chunk.meta.path.clone()).collect();
        let b: Vec<_> = blank.iter().map(|h| h.chunk.meta.path.clone()).collect();
        assert_eq!(a, b, "whitespace expansion must be a no-op");
    }

    #[test]
    fn trigram_channel_recalls_precise_cjk_phrase() {
        // Two CJK docs: one is about the exact phrase "用户鉴权码", the other
        // merely contains the characters scattered. The trigram channel should
        // help the exact-phrase doc rank, and a CJK phrase query must retrieve.
        let tmp = tempfile::TempDir::new().unwrap();
        let kd = tmp.path().join("knowledge");
        fs::create_dir_all(kd.join("security")).unwrap();
        fs::write(
            kd.join("security/auth.md"),
            "# 鉴权\n\n## 令牌\n\n用户鉴权码用于校验用户身份与会话令牌。",
        )
        .unwrap();
        let cfg = RetrievalConfig::default();
        let hits = retrieve(tmp.path(), &kd, &cfg, "用户鉴权码生成", Phase::Research);
        assert!(
            !hits.is_empty(),
            "CJK trigram phrase query must retrieve the phrase doc"
        );
        assert!(hits[0].chunk.meta.path.contains("auth"));
    }

    #[test]
    fn trigram_channel_is_identity_for_ascii_query() {
        // A non-CJK query produces no trigram terms → the trigram fusion is a
        // no-op and results match the pre-trigram bigram-only behaviour.
        let tmp = tempfile::TempDir::new().unwrap();
        let kd = seed_corpus(tmp.path());
        let index = load_or_build_index_multi(tmp.path(), std::slice::from_ref(&kd));
        let over_fetch = 12;
        let bigram = index.search("login oauth", over_fetch);
        let fused = fuse_trigram_channel(&index, "login oauth", bigram.clone(), over_fetch);
        assert_eq!(
            bigram, fused,
            "ASCII query trigram fusion must be byte-for-byte identity"
        );
    }

    #[test]
    fn rrf_fuse_respects_top_k() {
        let chunks = crate::chunker::chunk_text(
            "a.md",
            "# A\n\n## One\n\nx\n\n## Two\n\ny\n\n## Three\n\nz",
        );
        let index = Bm25Index::from_chunks(chunks);
        let bm25: Vec<(usize, f64)> = vec![(0, 3.0), (1, 2.0), (2, 1.0)];
        let vec_hits: Vec<(u32, f32)> = vec![(0, 0.9), (1, 0.7), (2, 0.5)];
        let fused = rrf_fuse(&index, &bm25, &vec_hits, 60, 2);
        assert_eq!(fused.len(), 2);
    }

    #[test]
    fn quality_boost_reorders_not_just_filters() {
        // MED #3: a curated chunk (high quality_score) with a slightly LOWER raw
        // score must rank ABOVE a neutral chunk with a higher raw score, because
        // `normalise` now RE-SORTS by the boosted score. Previously the boost was
        // attached but the order stayed raw-score, so curated docs never actually
        // ranked higher in ORDER.
        let a = crate::chunker::chunk_text("a.md", "# A\n\n## S\n\naaa body")[0].clone();
        let b = crate::chunker::chunk_text("b.md", "# B\n\n## S\n\nbbb body")[0].clone();
        let c = crate::chunker::chunk_text(
            "c.md",
            "---\nquality_score: 100\n---\n# C\n\n## S\n\nccc body",
        )[0]
        .clone();
        let index = Bm25Index::from_chunks(vec![a, b, c]); // idx 0=a, 1=b, 2=c
                                                           // Raw order a(1.0) > b(0.65) > c(0.63). The default score is neutral,
                                                           // while quality=100 contributes only 1.05×: c=0.6615, narrowly above b.
                                                           // A materially weaker hit would remain lower.
                                                           // Empty usefulness store → every weight neutral 1.0, so this is purely the
                                                           // quality-boost reorder (proving the prior is a no-op on a fresh corpus).
        let store = crate::usefulness::UsefulnessStore::default();
        let out = normalise(&index, vec![(0, 1.0), (1, 0.65), (2, 0.63)], &store);
        let paths: Vec<&str> = out.iter().map(|h| h.chunk.meta.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["a.md", "c.md", "b.md"],
            "quality boost must reorder the curated chunk c above b: {paths:?}"
        );
    }

    #[test]
    fn normalise_tiebreak_is_deterministic_by_index() {
        // Two chunks that tie on boosted score must order by ascending chunk_idx,
        // reproducibly (the crate's stated determinism).
        let a = crate::chunker::chunk_text("a.md", "# A\n\n## S\n\naaa")[0].clone();
        let b = crate::chunker::chunk_text("b.md", "# B\n\n## S\n\nbbb")[0].clone();
        let index = Bm25Index::from_chunks(vec![a, b]);
        let store = crate::usefulness::UsefulnessStore::default();
        for _ in 0..32 {
            // Equal raw score + equal (default) quality → equal boosted score.
            let out = normalise(&index, vec![(1, 0.5), (0, 0.5)], &store);
            let paths: Vec<&str> = out.iter().map(|h| h.chunk.meta.path.as_str()).collect();
            assert_eq!(paths, vec!["a.md", "b.md"], "lower chunk_idx wins the tie");
        }
    }

    #[test]
    fn normalise_preserves_close_score_order_without_saturation_ties() {
        let higher = crate::chunker::chunk_text(
            "z-higher.md",
            "---\nquality_score: 100\n---\n# Higher\n\n## S\n\nbody",
        )
        .remove(0);
        let lower = crate::chunker::chunk_text(
            "a-lower.md",
            "---\nquality_score: 100\n---\n# Lower\n\n## S\n\nbody",
        )
        .remove(0);
        let index = Bm25Index::from_chunks(vec![higher, lower]);
        let store = crate::usefulness::UsefulnessStore::default();
        let out = normalise(&index, vec![(0, 1.0), (1, 0.99)], &store);
        assert_eq!(out[0].chunk.meta.path, "z-higher.md");
        assert!((out[0].score - 1.0).abs() < f32::EPSILON);
        assert!(out[1].score < 1.0);
    }

    #[test]
    fn project_corpus_breaks_close_ties_but_not_material_relevance() {
        let mut bundled =
            crate::chunker::chunk_text("guide.md", "# Bundled\n\n## S\n\nbundled").remove(0);
        bundled.meta.corpus_origin = crate::CorpusOrigin::BundledCurated;
        let mut project =
            crate::chunker::chunk_text("guide.md", "# Project\n\n## S\n\nproject").remove(0);
        project.meta.corpus_origin = crate::CorpusOrigin::ProjectCustom;
        let index = Bm25Index::from_chunks(vec![bundled, project]);
        let store = crate::usefulness::UsefulnessStore::default();

        let tied = normalise(&index, vec![(0, 1.0), (1, 1.0)], &store);
        assert_eq!(tied[0].chunk.meta.title, "Project");

        let stronger_bundled = normalise(&index, vec![(0, 1.3), (1, 1.0)], &store);
        assert_eq!(stronger_bundled[0].chunk.meta.title, "Bundled");
    }

    #[test]
    fn usefulness_prior_lifts_proven_helpful_over_equal_unobserved() {
        // Three chunks: c is the clear top hit (so the normalised top stays 1.0),
        // a and b tie on raw relevance AND quality. a has a proven-helpful track
        // record; b is unobserved. The prior must break the tie in a's favour —
        // WITHOUT dropping b or disturbing the genuine top hit c.
        let a = crate::chunker::chunk_text("a.md", "# A\n\n## S\n\naaa body")[0].clone();
        let b = crate::chunker::chunk_text("b.md", "# B\n\n## S\n\nbbb body")[0].clone();
        let c = crate::chunker::chunk_text("c.md", "# C\n\n## S\n\nccc body")[0].clone();
        let index = Bm25Index::from_chunks(vec![a, b, c]); // idx 0=a, 1=b, 2=c
        let mut store = crate::usefulness::UsefulnessStore::default();
        for _ in 0..crate::usefulness::MIN_SAMPLES {
            store.record(&[("a.md".to_string(), "S".to_string())], true);
        }
        // c raw 10 (top), a and b raw 5 (equal, sub-max so no clamp collision).
        let out = normalise(&index, vec![(2, 10.0), (0, 5.0), (1, 5.0)], &store);
        let paths: Vec<&str> = out.iter().map(|h| h.chunk.meta.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["c.md", "a.md", "b.md"],
            "proven-helpful `a` must outrank equal-relevance unobserved `b`: {paths:?}"
        );
        assert!(
            (out[0].score - 1.0).abs() < 1e-5,
            "the genuine top hit stays normalised to 1.0"
        );
    }

    #[test]
    fn usefulness_prior_is_identity_on_a_fresh_corpus() {
        // With an EMPTY store, normalise must produce byte-for-byte the same
        // ordering + scores it produced before the prior existed (neutral 1.0).
        let a = crate::chunker::chunk_text("a.md", "# A\n\n## S\n\naaa body")[0].clone();
        let b = crate::chunker::chunk_text("b.md", "# B\n\n## S\n\nbbb body")[0].clone();
        let index = Bm25Index::from_chunks(vec![a, b]);
        let empty = crate::usefulness::UsefulnessStore::default();
        let out = normalise(&index, vec![(0, 1.0), (1, 0.6)], &empty);
        let paths: Vec<&str> = out.iter().map(|h| h.chunk.meta.path.as_str()).collect();
        assert_eq!(paths, vec!["a.md", "b.md"], "fresh corpus ranks as before");
        assert!((out[0].score - 1.0).abs() < 1e-5);
    }

    #[test]
    fn usefulness_prior_sinks_a_proven_harmful_chunk() {
        // a has the HIGHER raw relevance but a proven-unhelpful track record
        // (weight ≈ 0.6); b is unobserved (weight 1.0). The prior must pull a below
        // b in ORDER while both stay above the min_score gate — the prior demotes,
        // it never discards a relevant chunk.
        let a = crate::chunker::chunk_text("a.md", "# A\n\n## S\n\naaa body")[0].clone();
        let b = crate::chunker::chunk_text("b.md", "# B\n\n## S\n\nbbb body")[0].clone();
        let index = Bm25Index::from_chunks(vec![a, b]);
        let mut store = crate::usefulness::UsefulnessStore::default();
        // 1 helpful + 2 harmful → ratio 1/3 → weight ≈ 0.6 (well-sampled, sinks but
        // not to the floor, so a stays above the 0.5 min_score gate).
        store.record(&[("a.md".to_string(), "S".to_string())], true);
        store.record(&[("a.md".to_string(), "S".to_string())], false);
        store.record(&[("a.md".to_string(), "S".to_string())], false);
        // a raw 5.0 (top), b raw 4.9: base_a=1.0×1.25×0.6=0.75, base_b≈0.98×1.25=1.0.
        let out = normalise(&index, vec![(0, 5.0), (1, 4.9)], &store);
        let paths: Vec<&str> = out.iter().map(|h| h.chunk.meta.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["b.md", "a.md"],
            "proven-unhelpful `a` sinks below unobserved `b` yet is kept: {paths:?}"
        );
    }

    #[test]
    fn usefulness_memory_cannot_overturn_materially_stronger_current_evidence() {
        let strong =
            crate::chunker::chunk_text("strong.md", "# Strong\n\n## S\n\nexact current evidence")
                [0]
            .clone();
        let weak =
            crate::chunker::chunk_text("weak.md", "# Weak\n\n## S\n\nnearby evidence")[0].clone();
        let index = Bm25Index::from_chunks(vec![strong, weak]);
        let mut store = crate::usefulness::UsefulnessStore::default();
        for _ in 0..crate::usefulness::MIN_SAMPLES {
            store.record(&[("strong.md".into(), "S".into())], false);
            store.record(&[("weak.md".into(), "S".into())], true);
        }
        let out = normalise(&index, vec![(0, 10.0), (1, 9.0)], &store);
        let paths = out
            .iter()
            .map(|hit| hit.chunk.meta.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            paths.first(),
            Some(&"strong.md"),
            "even maximally opposed history must not beat an 11% relevance gap: {paths:?}"
        );
    }

    #[cfg(feature = "vector")]
    #[test]
    fn vector_channel_is_phase_filtered() {
        // MED #2: the vector channel must NOT reintroduce off-phase chunks the
        // BM25 phase filter excludes. In the Frontend phase a `design-systems/`
        // chunk is deliberately excluded (it's inlined as the binding contract,
        // not retrieved); even when the vector store ranks it #1 it must not
        // surface.
        let _env = crate::testsupport::env_guard();
        let prev_key = std::env::var("OPENAI_API_KEY").ok();
        std::env::set_var("OPENAI_API_KEY", "test-dummy-no-network");

        let tmp = tempfile::TempDir::new().unwrap();
        let kd = tmp.path().join("knowledge");
        fs::create_dir_all(kd.join("frontend")).unwrap();
        fs::create_dir_all(kd.join("design-systems")).unwrap();
        // In-phase doc the BM25 query hits (so the phase filter is non-empty and
        // does not fall back to unfiltered).
        fs::write(
            kd.join("frontend/components.md"),
            "# Components\n\n## Buttons\n\nbutton component primary variant states",
        )
        .unwrap();
        // Off-phase (Frontend) doc the BM25 query does NOT hit, but the vector
        // store will rank #1 for the query vector.
        fs::write(
            kd.join("design-systems/archetype.md"),
            "# Archetype\n\n## Palette\n\nbrandpalettetoken neutralscale elevation",
        )
        .unwrap();

        // Build the index the SAME way retrieve will, so chunk positions + the
        // fingerprint align.
        let index = load_or_build_index_multi(tmp.path(), &corpus_dirs(tmp.path(), &kd));
        let ds_idx = index
            .chunks
            .iter()
            .position(|c| c.meta.path.contains("design-systems"))
            .expect("design-systems chunk indexed");

        let qvec = vec![1.0f32, 0.0, 0.0, 0.0];
        let entries: Vec<(u32, String, String, u64, Vec<f32>)> = index
            .chunks
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let v = if i == ds_idx {
                    vec![1.0f32, 0.0, 0.0, 0.0]
                } else {
                    vec![0.0f32, 1.0, 0.0, 0.0]
                };
                (
                    u32::try_from(i).unwrap(),
                    c.meta.path.clone(),
                    c.meta.section.clone(),
                    0,
                    v,
                )
            })
            .collect();
        let mut store = vector::VectorStore::from_embedded("test-model", entries);
        // Stamp with the live fingerprint so the MED #4 alignment gate lets fusion run.
        store.set_corpus_sig(crate::index::corpus_fingerprint(&index));
        store.save(tmp.path());

        let cfg = RetrievalConfig {
            enabled: true,
            engine: RetrievalEngine::Hybrid,
            top_k: 5,
            custom_dirs: vec![],
        };
        let hits = retrieve_with_vector(
            tmp.path(),
            &kd,
            &cfg,
            "button component",
            Phase::Frontend,
            Some(&qvec),
        );
        let paths: Vec<&str> = hits.iter().map(|h| h.chunk.meta.path.as_str()).collect();
        assert!(
            paths.iter().any(|p| p.contains("frontend/components")),
            "the in-phase frontend chunk must surface: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.contains("design-systems")),
            "the off-phase design-systems chunk must NOT surface even though the vector store ranks it #1: {paths:?}"
        );

        match prev_key {
            Some(v) => std::env::set_var("OPENAI_API_KEY", v),
            None => std::env::remove_var("OPENAI_API_KEY"),
        }
    }

    #[cfg(feature = "vector")]
    #[test]
    fn vector_fusion_skipped_on_corpus_signature_mismatch() {
        // MED #4: when the cached vector store's corpus fingerprint does not match
        // the live index (the corpus shifted since the store was built), vector
        // fusion is skipped so a stale positional chunk_idx can't attribute a hit
        // to the WRONG chunk. Proven with a query that matches NOTHING in BM25: a
        // MATCHING fingerprint surfaces the vector-only chunk; a STALE one must not.
        let _env = crate::testsupport::env_guard();
        let prev_key = std::env::var("OPENAI_API_KEY").ok();
        std::env::set_var("OPENAI_API_KEY", "test-dummy-no-network");

        let tmp = tempfile::TempDir::new().unwrap();
        let kd = seed_corpus(tmp.path());
        let index = load_or_build_index_multi(tmp.path(), &corpus_dirs(tmp.path(), &kd));
        let login_idx = index
            .chunks
            .iter()
            .position(|c| c.meta.path.contains("login"))
            .expect("login chunk indexed");

        let qvec = vec![1.0f32, 0.0, 0.0, 0.0];
        let entries: Vec<(u32, String, String, u64, Vec<f32>)> = index
            .chunks
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let v = if i == login_idx {
                    vec![1.0f32, 0.0, 0.0, 0.0]
                } else {
                    vec![0.0f32, 1.0, 0.0, 0.0]
                };
                (
                    u32::try_from(i).unwrap(),
                    c.meta.path.clone(),
                    c.meta.section.clone(),
                    0,
                    v,
                )
            })
            .collect();

        let cfg = RetrievalConfig {
            enabled: true,
            engine: RetrievalEngine::Hybrid,
            top_k: 5,
            custom_dirs: vec![],
        };
        // The query matches NOTHING in BM25, so any hit can only come from vectors.
        let query = "zzzvectoronlyquery";

        // (A) MATCHING fingerprint → fusion runs → the vector-only chunk surfaces.
        let mut good = vector::VectorStore::from_embedded("m", entries.clone());
        good.set_corpus_sig(crate::index::corpus_fingerprint(&index));
        good.save(tmp.path());
        let hits_match =
            retrieve_with_vector(tmp.path(), &kd, &cfg, query, Phase::Research, Some(&qvec));
        assert!(
            hits_match
                .iter()
                .any(|h| h.chunk.meta.path.contains("login")),
            "matching fingerprint -> vector fusion surfaces the vector-only chunk: {:?}",
            hits_match
                .iter()
                .map(|h| &h.chunk.meta.path)
                .collect::<Vec<_>>()
        );

        // (B) STALE fingerprint → fusion skipped → BM25-only → nothing for a query
        // that matches nothing lexically.
        let mut stale = vector::VectorStore::from_embedded("m", entries);
        stale.set_corpus_sig("stale-fingerprint-that-will-not-match".into());
        stale.save(tmp.path());
        let hits_stale =
            retrieve_with_vector(tmp.path(), &kd, &cfg, query, Phase::Research, Some(&qvec));
        assert!(
            !hits_stale.iter().any(|h| h.chunk.meta.path.contains("login")),
            "stale fingerprint -> vector fusion skipped, the vector-only chunk must NOT surface: {:?}",
            hits_stale
                .iter()
                .map(|h| &h.chunk.meta.path)
                .collect::<Vec<_>>()
        );

        match prev_key {
            Some(v) => std::env::set_var("OPENAI_API_KEY", v),
            None => std::env::remove_var("OPENAI_API_KEY"),
        }
    }
}
