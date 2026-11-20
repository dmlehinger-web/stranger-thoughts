//! Static text for the "a stranger mailed a letter" note.
//!
//! The subject and the sentences are fixed. The only interpolated value is
//! the desk link, and that link is built from the configured site origin and
//! a role we already know. Caller-supplied names, emails, and letter text
//! are not parameters of this module.

use crate::authz::Role;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    LetterOnTheWay,
    ReplyOnTheWay,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailNotice {
    pub subject: String,
    pub text_body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NoticeError {
    #[error("bad desk link")]
    BadLink,
}

pub fn notice_kind_for_recipient(role: Role) -> NoticeKind {
    match role {
        Role::Reader => NoticeKind::LetterOnTheWay,
        Role::Writer => NoticeKind::ReplyOnTheWay,
    }
}

/// Build the plain-text note. `origin` is the site origin from configuration,
/// never a value a writer or reader typed into this request.
pub fn compose(kind: NoticeKind, origin: &str) -> Result<MailNotice, NoticeError> {
    let link = desk_link(origin, kind)?;
    let subject = match kind {
        NoticeKind::LetterOnTheWay => "A stranger marked a letter on the way",
        NoticeKind::ReplyOnTheWay => "A stranger marked a reply on the way",
    };
    let lead = match kind {
        NoticeKind::LetterOnTheWay => "A stranger marked a letter on the way.",
        NoticeKind::ReplyOnTheWay => "A stranger marked a reply on the way.",
    };
    let text_body = format!(
        "{lead}\n\nOpen your desk to see the status:\n{link}\n\nThis note does not include a name, an address, or the letter.\n"
    );
    if text_body.contains('\r') || subject.contains('\n') || subject.contains('\r') {
        return Err(NoticeError::BadLink);
    }
    Ok(MailNotice {
        subject: subject.to_string(),
        text_body,
    })
}

pub fn desk_link(origin: &str, kind: NoticeKind) -> Result<String, NoticeError> {
    if origin.contains(['\n', '\r', ' ', '\t', '@', '\\', '?', '#', '"', '<', '>']) {
        return Err(NoticeError::BadLink);
    }
    let trimmed = origin.trim_end_matches('/');
    if !trimmed.starts_with("https://") || trimmed["https://".len()..].contains('/') {
        return Err(NoticeError::BadLink);
    }
    let host = &trimmed["https://".len()..];
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return Err(NoticeError::BadLink);
    }
    let path = match kind {
        NoticeKind::LetterOnTheWay => "/read/home/",
        NoticeKind::ReplyOnTheWay => "/write/home/",
    };
    Ok(format!("{trimmed}{path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_is_static_and_has_no_sender_pii() {
        let note = compose(NoticeKind::LetterOnTheWay, "https://astrangerreads.com").unwrap();
        assert_eq!(note.subject, "A stranger marked a letter on the way");
        assert!(note.text_body.contains("https://astrangerreads.com/read/home/"));
        assert!(!note.text_body.contains('@'));
        assert!(!note.text_body.contains('<'));
        for banned in ["Cedar", "North Window", "line1", "example.com", "sk_live", "rk_live"] {
            assert!(!note.subject.contains(banned), "{banned}");
            assert!(!note.text_body.contains(banned), "{banned}");
        }
        let reply = compose(NoticeKind::ReplyOnTheWay, "https://astrangerreads.com").unwrap();
        assert!(reply.text_body.contains("/write/home/"));
        assert_eq!(reply.subject, "A stranger marked a reply on the way");
    }

    #[test]
    fn rejects_header_injection_and_userinfo() {
        assert!(compose(NoticeKind::LetterOnTheWay, "https://astrangerreads.com\nBcc: a@b.c").is_err());
        assert!(compose(NoticeKind::LetterOnTheWay, "https://user@astrangerreads.com").is_err());
        assert!(compose(NoticeKind::LetterOnTheWay, "http://astrangerreads.com").is_err());
        assert!(desk_link("https://astrangerreads.com/extra", NoticeKind::LetterOnTheWay).is_err());
    }
}
