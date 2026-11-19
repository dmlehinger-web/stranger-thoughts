use crate::authz::Role;

/// What an account wipe must remove. Payment rows stay, without an address,
/// so a refund or a dispute can still be reconciled. Pseudonyms on old
/// matches are replaced with [`REMOVED_PSEUDONYM`].
pub const REMOVED_PSEUDONYM: &str = "Removed";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionPlan {
    pub delete_profile: bool,
    pub delete_demographics: bool,
    pub delete_address_ciphertext: bool,
    pub deactivate_reader: bool,
    pub anonymize_pseudonym: bool,
}

pub fn plan_deletion(role: Role) -> DeletionPlan {
    match role {
        Role::Writer => DeletionPlan {
            delete_profile: true,
            delete_demographics: true,
            delete_address_ciphertext: false,
            deactivate_reader: false,
            anonymize_pseudonym: true,
        },
        Role::Reader => DeletionPlan {
            delete_profile: true,
            delete_demographics: false,
            delete_address_ciphertext: true,
            deactivate_reader: true,
            anonymize_pseudonym: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_wipe_includes_the_address_ciphertext() {
        let plan = plan_deletion(Role::Reader);
        assert!(plan.delete_address_ciphertext);
        assert!(plan.deactivate_reader);
        assert!(plan.anonymize_pseudonym);
        let writer = plan_deletion(Role::Writer);
        assert!(writer.delete_demographics);
        assert!(!writer.delete_address_ciphertext);
    }
}
