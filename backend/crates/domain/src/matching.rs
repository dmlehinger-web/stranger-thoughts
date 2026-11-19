use crate::ids::{ReaderId, WriterId};
use rand::seq::SliceRandom;
use rand::{CryptoRng, Rng};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// A reader the matcher is allowed to see.
///
/// These fields are the entire input to a draw. Profile descriptors live in
/// another module and are not readable from here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchCandidate {
    pub reader_id: ReaderId,
    pub pseudonym: String,
    pub active: bool,
    /// Writers this reader has blocked. This is a safety filter, applied
    /// before the draw, and it is not a profile descriptor.
    pub blocked_writer_ids: Vec<WriterId>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MatchError {
    #[error("no eligible reader")]
    NoEligibleReaders,
}

/// Pick one active reader uniformly, using a cryptographic RNG.
///
/// `exclude_reader` is the writer's current stranger, so rotate lands on
/// someone else when anyone else is available. Readers who blocked this
/// writer are skipped. Nothing else about the writer is consulted.
pub fn choose_reader<R: CryptoRng + Rng>(
    rng: &mut R,
    writer_id: &WriterId,
    candidates: &[MatchCandidate],
    exclude_reader: Option<&ReaderId>,
) -> Result<ReaderId, MatchError> {
    let mut seen = HashSet::new();
    let eligible: Vec<&MatchCandidate> = candidates
        .iter()
        .filter(|candidate| {
            candidate.active
                && !candidate.pseudonym.trim().is_empty()
                && exclude_reader != Some(&candidate.reader_id)
                && !candidate
                    .blocked_writer_ids
                    .iter()
                    .any(|blocked| blocked == writer_id)
                && seen.insert(candidate.reader_id.clone())
        })
        .collect();
    let chosen = eligible.choose(rng).ok_or(MatchError::NoEligibleReaders)?;
    Ok(chosen.reader_id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;
    use std::collections::BTreeSet;

    fn reader(n: u8) -> MatchCandidate {
        MatchCandidate {
            reader_id: ReaderId::new(format!("reader-{n}")).unwrap(),
            pseudonym: format!("Name {n}"),
            active: true,
            blocked_writer_ids: Vec::new(),
        }
    }

    #[test]
    fn candidate_fields_are_only_id_pseudonym_active_and_blocks() {
        let value = serde_json::to_value(reader(1)).unwrap();
        let keys: BTreeSet<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["active", "blocked_writer_ids", "pseudonym", "reader_id"]
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn source_does_not_reference_demographic_fields() {
        let src = include_str!("matching.rs");
        let src = src.split("mod tests").next().unwrap_or(src);
        for word in [
            "age_range",
            "economic",
            "demographic",
            "location_bucket",
            "postal",
            "line1",
            "mailing",
        ] {
            assert!(
                !src.contains(word),
                "matching.rs mentions {word}, which must not be a matching input"
            );
        }
        // Whole word, so this does not trip on unrelated identifiers.
        assert!(
            !src.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "race"),
            "matching.rs mentions race"
        );
    }

    #[test]
    fn distribution_is_roughly_uniform() {
        let candidates: Vec<_> = (1..=5).map(reader).collect();
        let writer = WriterId::new("writer-1").unwrap();
        let mut counts = [0u32; 5];
        let mut rng = ChaCha20Rng::seed_from_u64(42);
        for _ in 0..5_000 {
            let id = choose_reader(&mut rng, &writer, &candidates, None).unwrap();
            let index = id.as_str().as_bytes().last().unwrap() - b'1';
            counts[index as usize] += 1;
        }
        for count in counts {
            assert!((800..=1200).contains(&count), "count {count} left the band");
        }
    }

    #[test]
    fn skips_inactive_blocked_and_current() {
        let writer = WriterId::new("writer-1").unwrap();
        let mut pool = vec![reader(1), reader(2), reader(3)];
        pool[0].active = false;
        pool[1].blocked_writer_ids.push(writer.clone());
        let current = pool[2].reader_id.clone();
        let mut rng = ChaCha20Rng::seed_from_u64(7);
        let err = choose_reader(&mut rng, &writer, &pool, Some(&current)).unwrap_err();
        assert_eq!(err, MatchError::NoEligibleReaders);
    }

    #[test]
    fn same_seed_same_pick_regardless_of_unrelated_payload() {
        let writer = WriterId::new("writer-1").unwrap();
        let left = vec![reader(1), reader(2), reader(3), reader(4)];
        let right = left.clone();
        let mut rng_a = ChaCha20Rng::seed_from_u64(99);
        let mut rng_b = ChaCha20Rng::seed_from_u64(99);
        let a = choose_reader(&mut rng_a, &writer, &left, None).unwrap();
        let b = choose_reader(&mut rng_b, &writer, &right, None).unwrap();
        assert_eq!(a, b);
    }
}
