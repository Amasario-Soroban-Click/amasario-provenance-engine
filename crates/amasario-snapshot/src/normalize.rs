//! Canonical normalisation: the ordering and the digest that make two captures comparable.
//!
//! # Why ordering is a correctness property here
//!
//! `taxonomies/change-types.yaml` states the consequence of getting this wrong: a diff
//! "MUST NOT report a difference caused only by non-deterministic ordering or by an
//! unstable identifier". A diff is the artefact most likely to be consumed without review -
//! it is what a pipeline alerts on - so an ordering difference is not cosmetic noise; it is
//! a false alarm that a reader has no way to distinguish from a real change without opening
//! both snapshots. Canonical order is therefore applied to every set before a snapshot is
//! written or compared, and it is applied once, here, rather than at each use.
//!
//! The orderings are the ones the producing crates already define rather than new ones:
//! graph nodes use [`amasario_graph::nodes::canonical_order`], graph edges use the graph's
//! own relationship-then-target rank, evidence is sorted by identifier because records are
//! cited by identifier, and dependency entries are sorted by the subject they are about
//! before their relationship. A second opinion about ordering would be a second thing to
//! keep in step with the first.
//!
//! # Why the volatile fields are declared rather than fixed in prose
//!
//! `schema/snapshot.schema.json` excludes `capturedAt` from the content digest, "because
//! two captures of the same state at different times must compare equal", and it moves the
//! exclusion into the document itself: `volatileFields` is "declared rather than fixed in
//! prose so that a consumer can recompute the digest itself and get the same answer". That
//! sentence is the whole design. A digest a consumer cannot recompute is a digest the
//! consumer must trust, and a trust-based digest cannot detect the one thing it exists to
//! detect - a producer that summarised content it did not have.
//!
//! [`DEFAULT_VOLATILE_FIELDS`] is therefore the engine's declaration, it travels inside
//! the digest it describes, and [`validate_volatile_fields`] refuses a declaration that
//! names something that is not excluded or asks to exclude something that is not volatile.
//! Excluding a field that is part of the state would make two different analyses digest
//! equal, which is precisely the outcome the digest exists to prevent.

use amasario_core::{Digest, DigestAlgorithm, Result};

use crate::capture::Snapshot;
use crate::errors::SnapshotFailure;

/// The fields the engine excludes from `contentDigest`, as JSON pointers.
///
/// `capturedAt` is the schema's own example: it records when the capture ran, which is a
/// fact about the analyst rather than about the contract, and including it would make two
/// captures of one unchanged state different documents - so a diff would report a change
/// every time it was run, and the digest would answer "has anything changed" with "yes,
/// time passed".
///
/// `contentDigest` excludes itself. A digest cannot contain itself, and the field has to
/// be declared for the same reason as the other: a consumer recomputing the digest must
/// know to remove it rather than to expect it.
pub const DEFAULT_VOLATILE_FIELDS: [&str; 2] = ["/capturedAt", "/contentDigest"];

/// The fields that are part of the digested state, and therefore may not be declared
/// volatile.
///
/// The list is the set of top-level fields that describe *what was analysed* rather than
/// *when the analysis ran*. A declaration that excludes one of them would let two
/// different states produce one digest, which is the failure the digest exists to catch:
/// two snapshots of different contracts, or of the same contract at different boundaries,
/// would compare as unchanged.
pub const DIGESTED_STATE_FIELDS: [&str; 10] = [
    "/apiVersion",
    "/specVersion",
    "/boundary",
    "/network",
    "/ledgerBoundary",
    "/contract",
    "/wasm",
    "/evidence",
    "/confidence",
    "/truncated",
];

/// Checks a `volatileFields` declaration against what the engine actually excludes.
///
/// Every failure is a disagreement between the document's statement about its own digest
/// and the digest the engine computes. Both directions are refused: an undeclared
/// exclusion makes a consumer's recomputation differ from the engine's, and an unknown or
/// non-volatile declaration makes the consumer exclude something the engine did not.
#[must_use]
pub fn validate_volatile_fields(declared: &[String]) -> Vec<SnapshotFailure> {
    let mut failures: Vec<SnapshotFailure> = Vec::new();

    for required in DEFAULT_VOLATILE_FIELDS {
        if !declared.iter().any(|field| field == required) {
            failures.push(SnapshotFailure::VolatileFieldUndeclared {
                field: required.to_owned(),
            });
        }
    }
    for field in declared {
        if DIGESTED_STATE_FIELDS.contains(&field.as_str()) {
            failures.push(SnapshotFailure::VolatileFieldNotVolatile {
                field: field.clone(),
            });
        } else if !DEFAULT_VOLATILE_FIELDS.contains(&field.as_str()) {
            failures.push(SnapshotFailure::VolatileFieldUnknown {
                field: field.clone(),
            });
        }
    }

    failures
}

/// The key two entity references are ordered by.
///
/// Kind first because a report groups by kind, so grouping and ordering are then the same
/// traversal; identifier second because it is the only other thing an `EntityRef` has.
#[must_use]
pub fn entity_key(entity: &amasario_core::EntityRef) -> (String, String) {
    (entity.kind.as_str().to_owned(), entity.id.clone())
}

/// The key two dependency entries are ordered by.
///
/// A dependency is a statement about a subject, so the subject leads. The object and the
/// relationship follow, which makes the order total: no two distinct dependencies share a
/// key, because two entries with the same subject, object and relationship are the same
/// dependency recorded twice.
#[must_use]
pub fn dependency_key(
    dependency: &amasario_dependency::Dependency,
) -> (String, String, String, String, String) {
    let (subject_kind, subject_id) = entity_key(&dependency.subject);
    let (object_kind, object_id) = entity_key(&dependency.object);
    (
        subject_kind,
        subject_id,
        dependency.relationship.as_str().to_owned(),
        object_kind,
        object_id,
    )
}

/// Puts every ordering-sensitive part of a snapshot into canonical order.
///
/// Idempotent: applying it twice produces the same document as applying it once, which is
/// what lets [`canonical_order_failures`] check a loaded document by comparing it with its
/// own normalisation.
pub fn canonicalise(snapshot: &mut Snapshot) {
    snapshot
        .evidence
        .sort_by(|left, right| left.id.cmp(&right.id));
    snapshot
        .impact
        .sort_by(|left, right| left.id.cmp(&right.id));
    snapshot
        .attestations
        .sort_by(|left, right| left.id.cmp(&right.id));

    if let Some(dependencies) = snapshot.dependencies.as_mut() {
        dependencies.direct.sort_by_key(dependency_key);
        dependencies.transitive.sort_by_key(dependency_key);
        dependencies.unestablished.sort_by(|left, right| {
            entity_key(&left.object)
                .cmp(&entity_key(&right.object))
                .then_with(|| left.relationship.as_str().cmp(right.relationship.as_str()))
        });
        dependencies
            .cycles
            .sort_by(|left, right| left.entity_ids().cmp(&right.entity_ids()));
    }

    if let Some(graph) = snapshot.graph.as_mut() {
        graph.nodes.sort_by(amasario_graph::nodes::canonical_order);
        graph.edges.sort_by(|left, right| {
            left.source
                .cmp(&right.source)
                .then_with(|| {
                    amasario_graph::edges::relationship_rank(left.relationship).cmp(
                        &amasario_graph::edges::relationship_rank(right.relationship),
                    )
                })
                .then_with(|| left.target.cmp(&right.target))
        });
    }
}

/// Every way a snapshot's sets are not in canonical order.
///
/// Reported rather than repaired on load. Sorting a document a producer emitted would hide
/// that producer's disagreement with the specification, and the disagreement is exactly
/// what a reader needs to see: a snapshot whose evidence is not ordered by identifier was
/// written by something that does not normalise, and a diff over it would carry the
/// ordering as noise.
#[must_use]
pub fn canonical_order_failures(snapshot: &Snapshot) -> Vec<SnapshotFailure> {
    let mut failures: Vec<SnapshotFailure> = Vec::new();
    let mut normalised = snapshot.clone();
    canonicalise(&mut normalised);

    if normalised.evidence != snapshot.evidence {
        failures.push(SnapshotFailure::NotCanonicallyOrdered {
            section: "/evidence".to_owned(),
        });
    }
    if normalised.impact != snapshot.impact {
        failures.push(SnapshotFailure::NotCanonicallyOrdered {
            section: "/impact".to_owned(),
        });
    }
    if normalised.attestations != snapshot.attestations {
        failures.push(SnapshotFailure::NotCanonicallyOrdered {
            section: "/attestations".to_owned(),
        });
    }
    match (&normalised.dependencies, &snapshot.dependencies) {
        (Some(after), Some(before)) if after.direct != before.direct => {
            failures.push(SnapshotFailure::NotCanonicallyOrdered {
                section: "/dependencies/direct".to_owned(),
            });
        },
        (Some(after), Some(before)) if after.transitive != before.transitive => {
            failures.push(SnapshotFailure::NotCanonicallyOrdered {
                section: "/dependencies/transitive".to_owned(),
            });
        },
        _ => {},
    }
    match (&normalised.graph, &snapshot.graph) {
        (Some(after), Some(before)) if after.nodes != before.nodes => {
            failures.push(SnapshotFailure::NotCanonicallyOrdered {
                section: "/graph/nodes".to_owned(),
            });
        },
        (Some(after), Some(before)) if after.edges != before.edges => {
            failures.push(SnapshotFailure::NotCanonicallyOrdered {
                section: "/graph/edges".to_owned(),
            });
        },
        _ => {},
    }

    failures
}

/// A snapshot as JSON with every declared volatile field removed.
///
/// The removal is by JSON pointer, so it is the same operation a consumer performs to
/// recompute the digest: the engine does not have a privileged way of excluding a field
/// that the declaration does not describe.
///
/// # Errors
///
/// Returns a snapshot error when the snapshot cannot be serialised.
pub fn digested_value(snapshot: &Snapshot) -> Result<serde_json::Value> {
    let serialised = serde_json::to_value(snapshot).map_err(|error| {
        SnapshotFailure::Malformed {
            source: "the snapshot being digested".to_owned(),
            detail: error.to_string(),
        }
        .into_error()
    })?;
    let mut value = serialised;
    for field in &snapshot.volatile_fields {
        remove_pointer(&mut value, field);
    }
    Ok(value)
}

/// Removes the value at a JSON pointer, taking the first path segment only.
///
/// The declared fields are top-level by construction, and a pointer with more segments
/// would name a field inside a nested document that the declaration was never meant to
/// reach. Removing a segment that is absent is not an error: the field may legitimately be
/// omitted, and `volatileFields` declares what is excluded rather than what is present.
fn remove_pointer(value: &mut serde_json::Value, pointer: &str) {
    let Some(name) = pointer.trim_start_matches('/').split('/').next() else {
        return;
    };
    if let serde_json::Value::Object(object) = value {
        object.shift_remove(name);
    }
}

/// A snapshot's canonical JSON form, with the volatile fields removed.
///
/// This is the byte sequence the content digest is taken over. `serde_json`'s
/// `preserve_order` feature is what makes it canonical: fields are written in the order
/// the struct declares them rather than in a hash order that could differ between runs.
///
/// # Errors
///
/// Returns a snapshot error when the snapshot cannot be serialised.
pub fn canonical_json(snapshot: &Snapshot) -> Result<String> {
    let value = digested_value(snapshot)?;
    serde_json::to_string(&value).map_err(|error| {
        SnapshotFailure::Malformed {
            source: "the snapshot being written".to_owned(),
            detail: error.to_string(),
        }
        .into_error()
    })
}

/// The digest of a snapshot's canonical form.
///
/// # Errors
///
/// Returns a snapshot error when the snapshot cannot be serialised.
pub fn content_digest(snapshot: &Snapshot) -> Result<Digest> {
    canonical_json(snapshot).map(|json| Digest::sha256_of(json.as_bytes()))
}

/// Checks a recorded content digest against the content it summarises.
///
/// # Errors
///
/// Returns [`SnapshotFailure::DigestAlgorithmUnsupported`] when the recorded digest is not
/// SHA-256, and [`SnapshotFailure::ContentDigestDisagrees`] when it does not match.
pub fn verify_content_digest(snapshot: &Snapshot) -> Result<Digest> {
    if snapshot.content_digest.algorithm() != DigestAlgorithm::Sha256 {
        return Err(SnapshotFailure::DigestAlgorithmUnsupported {
            algorithm: snapshot.content_digest.algorithm().as_str().to_owned(),
        }
        .into_error());
    }
    let computed = content_digest(snapshot)?;
    if computed != snapshot.content_digest {
        return Err(SnapshotFailure::ContentDigestDisagrees {
            recorded: snapshot.content_digest.prefixed(),
            computed: computed.prefixed(),
        }
        .into_error());
    }
    Ok(computed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::tests_support::minimal;

    fn default_declaration() -> Vec<String> {
        DEFAULT_VOLATILE_FIELDS
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn the_default_declaration_names_the_fields_the_engine_excludes() {
        assert_eq!(
            DEFAULT_VOLATILE_FIELDS,
            ["/capturedAt", "/contentDigest"],
            "the declaration is the schema's own list, not a local convenience"
        );
        assert_eq!(
            validate_volatile_fields(&default_declaration()),
            Vec::new(),
            "the engine's own declaration must satisfy the engine"
        );
    }

    #[test]
    fn an_undeclared_exclusion_is_refused() {
        let failures = validate_volatile_fields(&["/contentDigest".to_owned()]);
        assert!(failures.iter().any(|failure| matches!(
            failure,
            SnapshotFailure::VolatileFieldUndeclared { field } if field == "/capturedAt"
        )));
    }

    #[test]
    fn excluding_the_contract_or_the_boundary_is_refused() {
        // Either exclusion would let two different analyses digest equal, which is the
        // failure the digest exists to catch.
        for field in ["/contract", "/boundary", "/network", "/ledgerBoundary"] {
            let mut declared = default_declaration();
            declared.push(field.to_owned());
            assert!(
                validate_volatile_fields(&declared)
                    .iter()
                    .any(|failure| matches!(
                        failure,
                        SnapshotFailure::VolatileFieldNotVolatile { field: f } if f == field
                    )),
                "{field} must not be declarable as volatile"
            );
        }
    }

    #[test]
    fn a_declaration_naming_a_field_that_is_not_excluded_is_refused() {
        let mut declared = default_declaration();
        declared.push("/impact".to_owned());
        assert!(
            validate_volatile_fields(&declared)
                .iter()
                .any(|failure| matches!(
                    failure,
                    SnapshotFailure::VolatileFieldUnknown { field } if field == "/impact"
                ))
        );
    }

    #[test]
    fn the_digest_is_stable_across_a_capture_time_change() {
        // The schema's own reason for excluding capturedAt: two captures of one state at
        // different times must compare equal.
        let first = minimal("2026-01-01T00:00:00Z");
        let second = minimal("2026-06-01T12:00:00Z");
        assert_eq!(
            canonical_json(&first).expect("serialises"),
            canonical_json(&second).expect("serialises"),
            "the capture time must not reach the digested form"
        );
        assert_eq!(
            content_digest(&first).expect("digests"),
            content_digest(&second).expect("digests")
        );
    }

    #[test]
    fn the_digest_changes_when_the_state_changes() {
        let first = minimal("2026-01-01T00:00:00Z");
        let mut second = minimal("2026-01-01T00:00:00Z");
        second.truncated = true;
        assert_ne!(
            content_digest(&first).expect("digests"),
            content_digest(&second).expect("digests"),
            "a bounded analysis is a different state from an exhaustive one"
        );
    }

    #[test]
    fn normalisation_is_idempotent_and_its_absence_is_detected() {
        let mut snapshot = minimal("2026-01-01T00:00:00Z");
        let mut extra = snapshot.evidence[0].clone();
        extra.id = "e-0".to_owned();
        snapshot.evidence.push(extra);
        assert!(
            !canonical_order_failures(&snapshot).is_empty(),
            "evidence out of identifier order is reported"
        );
        canonicalise(&mut snapshot);
        assert!(
            canonical_order_failures(&snapshot).is_empty(),
            "{:?}",
            canonical_order_failures(&snapshot)
        );
        let once = snapshot.clone();
        canonicalise(&mut snapshot);
        assert_eq!(snapshot, once, "canonicalisation is idempotent");
    }

    #[test]
    fn a_digest_recorded_under_another_algorithm_is_refused() {
        let mut snapshot = minimal("2026-01-01T00:00:00Z");
        snapshot.content_digest =
            Digest::new(DigestAlgorithm::Sha512, &"00".repeat(64)).expect("a well-formed digest");
        let error = verify_content_digest(&snapshot).expect_err("SHA-256 is the only algorithm");
        let message = error.to_string();
        assert!(message.contains("sha512"), "got: {message}");
        assert!(
            message.contains("defines SHA-256 only"),
            "the refusal must say which algorithm the specification does define: {message}"
        );
    }
}
