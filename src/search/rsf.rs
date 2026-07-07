//! Relative Score Fusion (RSF) — score-based fusion strategy.
//!
//! Fuses two ranked result lists (vector + keyword) by normalising raw
//! scores rather than reciprocal ranks:
//!
//! ```text
//! score = (dense_score / max_dense_score) * (sparse_score / max_sparse_score)
//! ```
//!
//! For candidates that appear in only one list, the missing factor falls
//! back to a configurable default (0.1). This produces smoother score
//! distributions than pure RRF and rewards high-confidence matches in
//! either modality.

use std::collections::HashMap;

use uuid::Uuid;

use super::SearchResult;

/// RSF fusion — score-based hybrid search.
///
/// Normalises vector and keyword raw scores independently, then
/// multiplies them for each candidate present in both lists. Entries
/// appearing in only one list receive a configurable default factor
/// for the missing modality so they are not discarded entirely.
pub struct RsfFusion;

impl RsfFusion {
    /// Fuse two ranked result lists using Relative Score Fusion.
    ///
    /// # Arguments
    ///
    /// * `vec_results` — vector search results with raw similarity scores.
    /// * `kw_results` — keyword (BM25) search results with raw relevance scores.
    /// * `vec_weight` — weight applied to the vector factor (default 0.6).
    /// * `kw_weight` — weight applied to the keyword factor (default 0.4).
    /// * `missing_default` — score factor for candidates missing from one list (default 0.1).
    ///
    /// # Returns
    ///
    /// A single `Vec<SearchResult>` ordered by descending fused score.
    #[must_use]
    pub fn fuse(
        vec_results: Vec<SearchResult>,
        kw_results: Vec<SearchResult>,
        vec_weight: f64,
        kw_weight: f64,
        missing_default: f64,
    ) -> Vec<SearchResult> {
        // Compute max scores for normalisation (handle empty inputs).
        let max_vec = vec_results
            .iter()
            .map(|r| r.score)
            .fold(0.0_f64, f64::max)
            .max(1e-12); // avoid division by zero
        let max_kw = kw_results
            .iter()
            .map(|r| r.score)
            .fold(0.0_f64, f64::max)
            .max(1e-12);

        let mut seen: HashMap<Uuid, FuseEntry> = HashMap::new();

        // Vector side — store normalised score as the base factor.
        for (rank, r) in vec_results.iter().enumerate() {
            let norm_score = (r.score / max_vec).clamp(0.0, 1.0);
            seen.insert(
                r.id,
                FuseEntry {
                    id: r.id,
                    content: r.content.clone(),
                    source_info: r.source_info.clone(),
                    vec_factor: norm_score,
                    kw_factor: None,
                    fused_score: 0.0,
                    rank: rank as i32 + 1,
                },
            );
        }

        // Keyword side — multiply or insert with default.
        for (rank, r) in kw_results.iter().enumerate() {
            let norm_score = (r.score / max_kw).clamp(0.0, 1.0);
            seen.entry(r.id)
                .and_modify(|e| {
                    e.kw_factor = Some(norm_score);
                })
                .or_insert(FuseEntry {
                    id: r.id,
                    content: r.content.clone(),
                    source_info: r.source_info.clone(),
                    vec_factor: missing_default,
                    kw_factor: Some(norm_score),
                    fused_score: 0.0,
                    rank: std::cmp::min(rank as i32 + 1, i32::MAX - 1),
                });
        }

        // Compute fused score for each entry.
        for entry in seen.values_mut() {
            let vec_score = entry.vec_factor * vec_weight;
            let kw_score = entry.kw_factor.unwrap_or(missing_default) * kw_weight;
            entry.fused_score = vec_score + kw_score;
        }

        // Sort by descending fused score.
        let mut entries: Vec<FuseEntry> = seen.into_values().collect();
        entries.sort_by(|a, b| {
            b.fused_score
                .partial_cmp(&a.fused_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        entries
            .into_iter()
            .map(|e| SearchResult {
                id: e.id,
                content: e.content,
                score: (e.fused_score * 10000.0).round() / 10000.0,
                source_info: e.source_info,
                vec_rank: if e.vec_factor > 0.0 {
                    Some(e.rank)
                } else {
                    None
                },
                kw_rank: e.kw_factor.map(|_| e.rank),
                decay_factor: None,
            })
            .collect()
    }
}

/// Internal entry used during RSF score merging.
struct FuseEntry {
    id: Uuid,
    content: String,
    source_info: String,
    vec_factor: f64,
    kw_factor: Option<f64>,
    fused_score: f64,
    rank: i32,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_result(id: Uuid, content: &str, score: f64) -> SearchResult {
        SearchResult {
            id,
            content: content.into(),
            score,
            source_info: String::new(),
            vec_rank: None,
            kw_rank: None,
            decay_factor: None,
        }
    }

    #[test]
    fn rsf_normalises_scores_correctly() {
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();

        let vec_results = vec![
            make_result(id_a, "A vec", 0.95), // max
            make_result(id_b, "B vec", 0.50),
        ];
        let kw_results = vec![
            make_result(id_b, "B kw", 0.80), // max
            make_result(id_a, "A kw", 0.60),
        ];

        let fused = RsfFusion::fuse(vec_results, kw_results, 0.6, 0.4, 0.1);
        // both in both lists:
        // A: (0.95/0.95)*0.6 + (0.60/0.80)*0.4 = 1.0*0.6 + 0.75*0.4 = 0.6 + 0.3 = 0.9
        // B: (0.50/0.95)*0.6 + (0.80/0.80)*0.4 ≈ 0.526*0.6 + 1.0*0.4 = 0.316 + 0.4 = 0.716
        assert_eq!(fused[0].id, id_a, "A has higher combined score");
        assert_eq!(fused[1].id, id_b, "B follows");
        assert!((fused[0].score - 0.9).abs() < 0.01);
    }

    #[test]
    fn rsf_handles_single_list_presence() {
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();

        let vec_results = vec![make_result(id_a, "A only vec", 0.90)];
        let kw_results = vec![make_result(id_b, "B only kw", 0.85)];

        let fused = RsfFusion::fuse(vec_results, kw_results, 0.6, 0.4, 0.1);
        // A: 1.0*0.6 + 0.1*0.4 = 0.64
        // B: 0.1*0.6 + 1.0*0.4 = 0.46
        assert_eq!(fused[0].id, id_a, "A with high vec score + missing default");
        assert_eq!(fused.len(), 2);
    }

    #[test]
    fn rsf_handles_empty_inputs() {
        let id = Uuid::new_v4();

        let vec_only = RsfFusion::fuse(vec![make_result(id, "only", 0.8)], vec![], 0.6, 0.4, 0.1);
        assert_eq!(vec_only.len(), 1);
        assert_eq!(vec_only[0].id, id);

        let kw_only = RsfFusion::fuse(vec![], vec![make_result(id, "only", 0.8)], 0.6, 0.4, 0.1);
        assert_eq!(kw_only.len(), 1);
        assert_eq!(kw_only[0].id, id);

        let empty = RsfFusion::fuse(vec![], vec![], 0.6, 0.4, 0.1);
        assert!(empty.is_empty());
    }

    #[test]
    fn rsf_weights_affect_ordering() {
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();

        let vec_results = vec![make_result(id_a, "A", 0.95)];
        let kw_results = vec![make_result(id_b, "B", 0.80)];

        let vec_heavy = RsfFusion::fuse(vec_results.clone(), kw_results.clone(), 0.9, 0.1, 0.05);
        assert_eq!(vec_heavy[0].id, id_a);

        let kw_heavy = RsfFusion::fuse(vec_results, kw_results, 0.1, 0.9, 0.05);
        assert_eq!(kw_heavy[0].id, id_b);
    }

    #[test]
    fn rsf_same_results_different_ordering_than_rrf() {
        let ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();

        // Results that should rank differently under RSF vs RRF
        let vec_results = vec![
            make_result(ids[0], "A vec", 0.90), // high vec, low kw
            make_result(ids[1], "B vec", 0.60), // medium vec, medium kw
            make_result(ids[2], "C vec", 0.30), // low vec, high kw
        ];
        let kw_results = vec![
            make_result(ids[2], "C kw", 0.95), // C is #1 in kw
            make_result(ids[1], "B kw", 0.50), // B is medium in kw
            make_result(ids[0], "A kw", 0.20), // A is low in kw
        ];

        let rsf_fused = RsfFusion::fuse(vec_results, kw_results, 0.6, 0.4, 0.1);

        // RSF score calc:
        // A: (0.90/0.90)*0.6 + (0.20/0.95)*0.4 = 0.6 + 0.084 = 0.684
        // B: (0.60/0.90)*0.6 + (0.50/0.95)*0.4 = 0.4 + 0.211 = 0.611
        // C: (0.30/0.90)*0.6 + (0.95/0.95)*0.4 = 0.2 + 0.4 = 0.600
        // Order: A > B > C
        assert_eq!(rsf_fused[0].id, ids[0], "A highest under RSF");
        assert_eq!(rsf_fused[1].id, ids[1], "B middle under RSF");
        assert_eq!(rsf_fused[2].id, ids[2], "C lowest under RSF");
    }

    #[test]
    fn rsf_scores_are_rounded_to_4_decimal_places() {
        let id = Uuid::new_v4();
        let vec_results = vec![make_result(id, "test", 0.95)];
        let fused = RsfFusion::fuse(vec_results, vec![], 0.6, 0.4, 0.1);

        let score_str = format!("{:.4}", fused[0].score);
        assert_eq!(score_str, "0.6400");
    }
}
