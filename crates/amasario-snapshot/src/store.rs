//! Reading and writing snapshots.
//!
//! # Why reading validates rather than trusting
//!
//! A snapshot is read back by a run that did not produce it - a later comparison, a
//! colleague's analysis, a pipeline step days later - and nothing about the file says
//! whether it was written by this engine or edited by hand. The read path therefore runs the
//! schema's requirements rather than assuming them, and the ones it can check without the
//! network are exactly the ones that matter: the identifier must be the one the contract and
//! boundary derive, the boundary echo must agree with the boundary, the sets must be in
//! canonical order, and the content digest must match the content it summarises.
//!
//! Canonical order is checked rather than applied on read. Sorting a document a producer
//! emitted would hide that producer's disagreement with the specification, and the
//! disagreement is what a reader needs: a snapshot whose evidence is not ordered by
//! identifier was written by something that does not normalise, and a diff over it would
//! carry the ordering as noise. The refusal names the section so the producer can fix it.
//!
//! # Why the file name is derived from the identifier
//!
//! The identifier is stable given the contract and the boundary, so deriving the file name
//! from it gives every state of one contract at one boundary one file. Re-capturing
//! overwrites rather than accumulating, which is what makes a snapshot directory a set of
//! states rather than a log of runs.
//!
//! # What is written
//!
//! The whole document, volatile fields included. It is tempting to write the digested form -
//! it is the canonical one, and it is what the digest is taken over - but the digested form
//! has the capture time removed, and a document that cannot say when it was captured cannot
//! be audited. The digest is a property of the content, not a replacement for it.

use std::path::{Path, PathBuf};

use amasario_core::Result;

use crate::capture::{SNAPSHOT_ID_PREFIX, Snapshot};
use crate::errors::SnapshotFailure;

/// The extension every stored snapshot carries.
pub const SNAPSHOT_EXTENSION: &str = "json";

/// The file name a snapshot is stored under.
///
/// Derived from the identifier rather than from a timestamp, so that re-capturing one state
/// replaces its file instead of adding a second copy of the same state under a new name.
#[must_use]
pub fn file_name(snapshot: &Snapshot) -> String {
    let stem = if snapshot.id.is_empty() {
        SNAPSHOT_ID_PREFIX.to_owned()
    } else {
        snapshot.id.clone()
    };
    format!("{stem}.{SNAPSHOT_EXTENSION}")
}

/// A snapshot as the JSON document that is written to disk.
///
/// # Errors
///
/// Returns a snapshot error when the snapshot cannot be serialised, which would be a defect
/// in a producing crate rather than in the input.
pub fn to_json(snapshot: &Snapshot) -> Result<String> {
    serde_json::to_string(snapshot).map_err(|error| {
        SnapshotFailure::Malformed {
            source: snapshot.id.clone(),
            detail: error.to_string(),
        }
        .into_error()
    })
}

/// A snapshot as indented JSON, for a document a person reads.
///
/// # Errors
///
/// As [`to_json`].
pub fn to_json_pretty(snapshot: &Snapshot) -> Result<String> {
    serde_json::to_string_pretty(snapshot).map_err(|error| {
        SnapshotFailure::Malformed {
            source: snapshot.id.clone(),
            detail: error.to_string(),
        }
        .into_error()
    })
}

/// Parses a snapshot and validates it.
///
/// `source` names where the text came from, so that a refusal points at a file rather than
/// at a string with no origin.
///
/// # Errors
///
/// Returns a malformed-snapshot error when the text is not a snapshot document, and the
/// first schema failure when it parses but disagrees with a requirement.
pub fn from_json(text: &str, source: &str) -> Result<Snapshot> {
    let snapshot: Snapshot = serde_json::from_str(text).map_err(|error| {
        SnapshotFailure::Malformed {
            source: source.to_owned(),
            detail: error.to_string(),
        }
        .into_error()
    })?;
    snapshot.validate()?;
    Ok(snapshot)
}

/// Writes a snapshot, creating the directory if needed.
///
/// # Errors
///
/// Returns a malformed-snapshot error when the file cannot be written, naming the path so
/// that the caller does not have to reconstruct it from the identifier.
pub fn write(snapshot: &Snapshot, directory: impl AsRef<Path>) -> Result<PathBuf> {
    let directory = directory.as_ref();
    std::fs::create_dir_all(directory).map_err(|error| {
        SnapshotFailure::Malformed {
            source: directory.display().to_string(),
            detail: format!("the directory could not be created: {error}"),
        }
        .into_error()
    })?;
    let path = directory.join(file_name(snapshot));
    let text = to_json_pretty(snapshot)?;
    std::fs::write(&path, text).map_err(|error| {
        SnapshotFailure::Malformed {
            source: path.display().to_string(),
            detail: format!("the snapshot could not be written: {error}"),
        }
        .into_error()
    })?;
    Ok(path)
}

/// Reads and validates a stored snapshot.
///
/// # Errors
///
/// Returns a malformed-snapshot error when the file cannot be read, and the first schema
/// failure when it reads but disagrees with a requirement.
pub fn read(path: impl AsRef<Path>) -> Result<Snapshot> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|error| {
        SnapshotFailure::Malformed {
            source: path.display().to_string(),
            detail: format!("the snapshot could not be read: {error}"),
        }
        .into_error()
    })?;
    from_json(&text, &path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::tests_support::minimal;
    use crate::normalize::canonicalise;

    #[test]
    fn a_snapshot_round_trips_through_a_file() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let snapshot = minimal("2026-01-01T00:00:00Z");
        let path = write(&snapshot, directory.path()).expect("written");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(file_name(&snapshot).as_str())
        );
        let restored = read(&path).expect("read back");
        assert_eq!(restored, snapshot);
    }

    #[test]
    fn the_capture_time_survives_the_round_trip_even_though_it_is_not_digested() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let snapshot = minimal("2026-03-04T05:06:07Z");
        let path = write(&snapshot, directory.path()).expect("written");
        let restored = read(&path).expect("read back");
        assert_eq!(restored.captured_at, "2026-03-04T05:06:07Z");
        assert!(
            restored.volatile_fields.contains(&"/capturedAt".to_owned()),
            "the exclusion is declared in the document itself, not only in code"
        );
    }

    #[test]
    fn re_capturing_one_state_replaces_its_file() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let first = write(&minimal("2026-01-01T00:00:00Z"), directory.path()).expect("written");
        let second = write(&minimal("2026-06-01T00:00:00Z"), directory.path()).expect("written");
        assert_eq!(first, second, "one state at one boundary is one file");
        let entries = std::fs::read_dir(directory.path()).expect("a readable directory");
        assert_eq!(entries.count(), 1);
    }

    #[test]
    fn a_hand_edited_identifier_is_refused_on_read() {
        let snapshot = minimal("2026-01-01T00:00:00Z");
        let text = to_json(&snapshot)
            .expect("serialises")
            .replace(&snapshot.id, "snapshot-edited");
        let error = from_json(&text, "in memory").expect_err("the identifier is derived");
        assert!(
            error.to_string().contains("snapshot-edited"),
            "got: {error}"
        );
    }

    #[test]
    fn a_reordered_set_is_refused_on_read_rather_than_sorted_silently() {
        let mut snapshot = minimal("2026-01-01T00:00:00Z");
        let mut extra = snapshot.evidence[0].clone();
        extra.id = "e-0".to_owned();
        snapshot.evidence.push(extra);
        // Canonical order is now violated, and the digest must be recomputed so that the
        // refusal reported is the ordering one rather than a digest disagreement.
        snapshot.content_digest = crate::normalize::content_digest(&snapshot).expect("digests");
        let error = from_json(&to_json(&snapshot).expect("serialises"), "in memory")
            .expect_err("an unordered set is a producer's disagreement, not a detail to fix");
        assert!(error.to_string().contains("/evidence"), "got: {error}");

        canonicalise(&mut snapshot);
        snapshot.content_digest = crate::normalize::content_digest(&snapshot).expect("digests");
        from_json(&to_json(&snapshot).expect("serialises"), "in memory")
            .expect("the normalised document is accepted");
    }

    #[test]
    fn a_truncated_document_is_refused_with_what_the_parser_saw() {
        let error = from_json("{\"apiVersion\":", "broken.json").expect_err("not JSON");
        let message = error.to_string();
        assert!(message.contains("broken.json"), "got: {message}");
    }

    #[test]
    fn a_missing_file_names_the_path_it_looked_for() {
        let error = read("/nonexistent/amasario/snapshot.json").expect_err("no such file");
        assert!(
            error
                .to_string()
                .contains("/nonexistent/amasario/snapshot.json"),
            "got: {error}"
        );
    }
}
