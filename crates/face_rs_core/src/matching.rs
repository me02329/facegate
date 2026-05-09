/// L2-normalized embedding vector produced by ArcFace-compatible models.
pub type Embedding = Vec<f32>;

/// Cosine similarity between two embedding vectors.
/// Both should be L2-normalized; result is in [-1.0, 1.0].
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// Returns the best (highest) similarity score against all enrolled embeddings.
pub fn best_similarity(current: &[f32], enrolled: &[Embedding]) -> Option<f32> {
    enrolled
        .iter()
        .map(|known| cosine_similarity(current, known))
        .reduce(f32::max)
}

/// True if any enrolled embedding matches the current one above the threshold.
pub fn is_match(current: &[f32], enrolled: &[Embedding], threshold: f32) -> bool {
    enrolled
        .iter()
        .any(|known| cosine_similarity(current, known) >= threshold)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_vec(v: &[f32]) -> Vec<f32> {
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / norm).collect()
    }

    #[test]
    fn identical_vectors_score_one() {
        let v = unit_vec(&[1.0, 2.0, 3.0]);
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn orthogonal_vectors_score_zero() {
        let a = unit_vec(&[1.0, 0.0, 0.0]);
        let b = unit_vec(&[0.0, 1.0, 0.0]);
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn match_above_threshold() {
        let enrolled = vec![unit_vec(&[1.0, 0.1, 0.0])];
        let current = unit_vec(&[1.0, 0.2, 0.0]);
        assert!(is_match(&current, &enrolled, 0.55));
    }

    #[test]
    fn no_match_below_threshold() {
        let enrolled = vec![unit_vec(&[1.0, 0.0, 0.0])];
        let current = unit_vec(&[0.0, 1.0, 0.0]);
        assert!(!is_match(&current, &enrolled, 0.55));
    }

    #[test]
    fn empty_enrolled_never_matches() {
        let current = unit_vec(&[1.0, 0.0, 0.0]);
        assert!(!is_match(&current, &[], 0.0));
        assert!(best_similarity(&current, &[]).is_none());
    }

    #[test]
    fn mismatched_dimensions_do_not_match_by_prefix() {
        let a = unit_vec(&[1.0, 0.0]);
        let b = unit_vec(&[1.0, 0.0, 0.0]);
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }
}
