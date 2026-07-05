//! Reciprocal Rank Fusion (RRF) standalone utility.
//!
//! Fuses two ranked result lists (vector + keyword) into a single
//! relevance-ordered list using the RRF formula:
//!
//! ```text
//! score = 1.0 / (k + rank + 1) * weight
//! ```
//!
//! Results appearing in both lists accumulate scores from both rankings.

use std::collections::HashMap;

use uuid::Uuid;

use super::SearchResult;

/// Standalone RRF fusion utility.
///
/// Takes two independently-ranked result lists and merges them via
/// Reciprocal Rank Fusion. Results that appear in both lists receive
/// the sum of their vector and keyword RRF scores.
pub struct RrfFusion;

impl RrfFusion {
    /// Fuse two ranked result lists into a single relevance-ordered list.
    ///
    /// # Arguments
    ///
    /// * `vec_results` — vector search results, ordered by descending similarity.
    /// * `kw_results` — keyword (BM25) search results, ordered by descending relevance.
    /// * `k` — RRF constant (typically 20 or 60). Larger values dampen rank differences.
    /// * `vec_weight` — weight applied to vector-side RRF scores.
    /// * `kw_weight` — weight applied to keyword-side RRF scores.
    ///
    /// # Returns
    ///
    /// A single `Vec<SearchResult>` ordered by descending fused score.
    /// Each result carries `vec_rank` and `kw_rank` when it appeared in
    /// the corresponding input list.
    #[must_use]
    pub fn fuse(
        vec_results: Vec<SearchResult>,
        kw_results: Vec<SearchResult>,
        k: u32,
        vec_weight: f64,
        kw_weight: f64,
    ) -> Vec<SearchResult> {
        let k_f64 = f64::from(k);
        let mut seen: HashMap<Uuid, FuseEntry> = HashMap::new();

        // Vector side
        for (rank, r) in vec_results.iter().enumerate() {
            let rrf_score = 1.0 / (k_f64 + rank as f64 + 1.0) * vec_weight;
            seen.insert(
                r.id,
                FuseEntry {
                    id: r.id,
                    content: r.content.clone(),
                    source_info: r.source_info.clone(),
                    rrf_score,
                    vec_rank: Some(rank as i32 + 1),
                    kw_rank: None,
                },
            );
        }

        // Keyword side — accumulate for shared IDs
        for (rank, r) in kw_results.iter().enumerate() {
            let kw_score = 1.0 / (k_f64 + rank as f64 + 1.0) * kw_weight;
            seen.entry(r.id)
                .and_modify(|e| {
                    e.rrf_score += kw_score;
                    e.kw_rank = Some(rank as i32 + 1);
                })
                .or_insert(FuseEntry {
                    id: r.id,
                    content: r.content.clone(),
                    source_info: r.source_info.clone(),
                    rrf_score: kw_score,
                    vec_rank: None,
                    kw_rank: Some(rank as i32 + 1),
                });
        }

        // Sort by descending fused score
        let mut entries: Vec<FuseEntry> = seen.into_values().collect();
        entries.sort_by(|a, b| {
            b.rrf_score
                .partial_cmp(&a.rrf_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        entries
            .into_iter()
            .map(|e| SearchResult {
                id: e.id,
                content: e.content,
                score: (e.rrf_score * 10000.0).round() / 10000.0,
                source_info: e.source_info,
                vec_rank: e.vec_rank,
                kw_rank: e.kw_rank,
                decay_factor: None,
            })
            .collect()
    }
}

/// Internal entry used during RRF rank merging.
struct FuseEntry {
    id: Uuid,
    content: String,
    source_info: String,
    rrf_score: f64,
    vec_rank: Option<i32>,
    kw_rank: Option<i32>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_result(id: Uuid, content: &str, source_info: &str) -> SearchResult {
        SearchResult {
            id,
            content: content.into(),
            score: 0.0,
            source_info: source_info.into(),
            vec_rank: None,
            kw_rank: None,
            decay_factor: None,
        }
    }

    // ------------------------------------------------------------------
    // RRF formula correctness
    // ------------------------------------------------------------------

    #[test]
    fn rrf_formula_matches_expected() {
        // Formula: 1.0 / (k + rank + 1) * weight
        let k = 20;
        let weight = 0.6;
        let rank = 0; // first result
        let expected = 1.0 / (k as f64 + rank as f64 + 1.0) * weight;
        // 1.0 / 21 * 0.6 ≈ 0.028571...
        assert!((expected - 0.028571428).abs() < 0.001);
    }

    #[test]
    fn rrf_score_decreases_with_rank() {
        let k = 20;
        let weight = 1.0;
        let score_r0 = 1.0 / (k as f64 + 0.0 + 1.0) * weight;
        let score_r1 = 1.0 / (k as f64 + 1.0 + 1.0) * weight;
        let score_r2 = 1.0 / (k as f64 + 2.0 + 1.0) * weight;
        assert!(score_r0 > score_r1);
        assert!(score_r1 > score_r2);
    }

    #[test]
    fn larger_k_dampens_rank_differences() {
        let _weight = 1.0;
        let k_small = 5;
        let k_large = 60;

        let diff_small = 1.0 / (k_small as f64 + 0.0 + 1.0) - 1.0 / (k_small as f64 + 1.0 + 1.0);
        let diff_large = 1.0 / (k_large as f64 + 0.0 + 1.0) - 1.0 / (k_large as f64 + 1.0 + 1.0);
        assert!(
            diff_small > diff_large,
            "larger k should produce smaller rank-to-rank differences"
        );
    }

    // ------------------------------------------------------------------
    // Fusion ordering
    // ------------------------------------------------------------------

    #[test]
    fn fuse_orders_by_descending_score() {
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();
        let id_c = Uuid::new_v4();

        // A is #1 in vector, C is #1 in keyword
        let vec_results = vec![make_result(id_a, "A", "vec"), make_result(id_b, "B", "vec")];
        let kw_results = vec![make_result(id_c, "C", "kw"), make_result(id_b, "B", "kw")];

        let fused = RrfFusion::fuse(vec_results, kw_results, 20, 0.6, 0.4);

        // B appears in both lists → highest fused score
        assert_eq!(fused[0].id, id_b, "shared result should rank first");
        // B should have both ranks
        assert_eq!(fused[0].vec_rank, Some(2));
        assert_eq!(fused[0].kw_rank, Some(2));
    }

    #[test]
    fn fuse_preserves_all_unique_ids() {
        let ids: Vec<Uuid> = (0..5).map(|_| Uuid::new_v4()).collect();

        let vec_results = vec![
            make_result(ids[0], "v0", "vec"),
            make_result(ids[1], "v1", "vec"),
        ];
        let kw_results = vec![
            make_result(ids[2], "k0", "kw"),
            make_result(ids[3], "k1", "kw"),
            make_result(ids[4], "k2", "kw"),
        ];

        let fused = RrfFusion::fuse(vec_results, kw_results, 20, 0.6, 0.4);
        assert_eq!(fused.len(), 5, "all unique IDs should be present");
    }

    #[test]
    fn fuse_handles_empty_inputs() {
        let id = Uuid::new_v4();

        // Only vector results
        let vec_only = RrfFusion::fuse(vec![make_result(id, "only", "vec")], vec![], 20, 0.6, 0.4);
        assert_eq!(vec_only.len(), 1);
        assert_eq!(vec_only[0].id, id);
        assert_eq!(vec_only[0].vec_rank, Some(1));
        assert_eq!(vec_only[0].kw_rank, None);

        // Only keyword results
        let kw_only = RrfFusion::fuse(vec![], vec![make_result(id, "only", "kw")], 20, 0.6, 0.4);
        assert_eq!(kw_only.len(), 1);
        assert_eq!(kw_only[0].id, id);
        assert_eq!(kw_only[0].vec_rank, None);
        assert_eq!(kw_only[0].kw_rank, Some(1));

        // Both empty
        let empty = RrfFusion::fuse(vec![], vec![], 20, 0.6, 0.4);
        assert!(empty.is_empty());
    }

    #[test]
    fn fuse_weights_affect_ordering() {
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();

        // A is #1 in vector, B is #1 in keyword — no overlap
        let vec_results = vec![make_result(id_a, "A", "vec")];
        let kw_results = vec![make_result(id_b, "B", "kw")];

        // Vector-heavy: A should win
        let vec_heavy = RrfFusion::fuse(vec_results.clone(), kw_results.clone(), 20, 0.9, 0.1);
        assert_eq!(vec_heavy[0].id, id_a);

        // Keyword-heavy: B should win
        let kw_heavy = RrfFusion::fuse(vec_results, kw_results, 20, 0.1, 0.9);
        assert_eq!(kw_heavy[0].id, id_b);
    }

    #[test]
    fn fuse_scores_are_rounded_to_4_decimal_places() {
        let id = Uuid::new_v4();
        let vec_results = vec![make_result(id, "test", "vec")];
        let fused = RrfFusion::fuse(vec_results, vec![], 20, 0.6, 0.4);

        // score = 1.0/21 * 0.6 = 0.028571428... → rounded to 0.0286
        let score_str = format!("{:.4}", fused[0].score);
        assert_eq!(score_str, "0.0286");
    }
}
