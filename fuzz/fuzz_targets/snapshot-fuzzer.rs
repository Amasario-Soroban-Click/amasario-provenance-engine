//! Fuzzes snapshot decoding, canonicalisation, content digests and comparison.
//!
//! # Why the decoder is the target rather than the writer
//!
//! A snapshot is the one engine artefact that is kept and read back, and it is read back
//! from a file that may have been edited, truncated, written by an older engine, or
//! written by something that is not the engine at all. The reader therefore takes
//! untrusted input, and everything here is about what it must do with it:
//!
//! * refuse a document that is not a snapshot, rather than partially accepting it;
//! * refuse a document that disagrees with a requirement, naming the first disagreement;
//! * refuse a document whose recorded digest disagrees with its contents, because a
//!   snapshot that verifies against its own digest is the basis of every comparison;
//! * never panic, whatever the bytes are.
//!
//! # Canonicalisation is a property, not a step
//!
//! Comparing two snapshots whose collections are in different orders must find no
//! difference, because the specification requires canonical comparison of the published
//! result. The target therefore checks that comparing a snapshot with a *shuffled copy of
//! itself* reports nothing - a comparison that depended on input order would report every
//! collection as replaced, and no unit test with a hand-written order would catch it.
//!
//! # Why one input is compared with two halves of itself
//!
//! The target reads the same bytes twice, once whole and once truncated, and compares
//! whatever parsed. That is the realistic case - a stored snapshot against a newer one -
//! and it is what exercises the diff's own rules: unique change identifiers, a reason on
//! every entry, and a summary that counts what the entries actually are.

#![no_main]

use libfuzzer_sys::fuzz_target;

use amasario_snapshot::{Snapshot, canonicalise, compare, from_json, to_json};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    // -- decoding, which must refuse rather than partially accept ------------
    let Ok(snapshot) = from_json(text, "fuzz-input") else {
        // A refusal is the expected outcome for most inputs. What matters is that it is
        // a refusal: reaching here means no panic occurred inside the derived
        // deserialiser or the validator.
        return;
    };
    snapshot
        .validate()
        .expect("a snapshot that parsed but does not validate must have been refused");

    // -- round trip, which must preserve what it read ------------------------
    let json = to_json(&snapshot).expect("a snapshot the engine accepted must serialise");
    let reparsed: Snapshot =
        from_json(&json, "fuzz-round-trip").expect("a snapshot the engine wrote must parse");
    assert_eq!(
        reparsed, snapshot,
        "a snapshot does not survive being written and read back"
    );

    // -- canonicalisation, which must not depend on the input order ----------
    let mut shuffled = snapshot.clone();
    shuffled.evidence.reverse();
    shuffled.impact.reverse();
    shuffled.attestations.reverse();
    if let Some(dependencies) = shuffled.dependencies.as_mut() {
        dependencies.direct.reverse();
        dependencies.transitive.reverse();
    }
    canonicalise(&mut shuffled);

    let mut canonical = snapshot.clone();
    canonicalise(&mut canonical);
    assert_eq!(
        canonical, shuffled,
        "canonicalisation depends on the order of the collections it sorts"
    );

    let identical = compare(&snapshot, &shuffled, "2026-01-02T00:00:00Z")
        .expect("a snapshot is comparable with a copy of itself");
    assert!(
        identical.changes.is_empty(),
        "a reordered copy of a snapshot was reported as different: {:?}",
        identical
            .changes
            .iter()
            .map(|change| change.id.as_str())
            .collect::<Vec<_>>()
    );

    // -- comparison against a truncated reading of the same bytes ------------
    //
    // The two snapshots come from the same document, so the second is either identical
    // or a snapshot the reader accepted on its own terms. Comparing them exercises the
    // diff over every category at once, and the diff's own rules are then asserted.
    for split in [0_usize, data.len() / 4, data.len() / 2, data.len()] {
        let half = &data[..split];
        let Ok(half_text) = std::str::from_utf8(half) else {
            continue;
        };
        let Ok(other) = from_json(half_text, "fuzz-input-truncated") else {
            continue;
        };

        let Ok(diff) = compare(&snapshot, &other, "2026-01-02T00:00:00Z") else {
            // An incomparable pair is a legitimate result: two snapshots of different
            // contracts, or of one contract at different networks. The engine must say
            // why rather than compare them anyway.
            continue;
        };

        let failures = diff.failures();
        assert!(
            failures.is_empty(),
            "the engine produced a diff that violates its own rules: {failures:?}"
        );

        let mut identifiers: Vec<&str> = diff.changes.iter().map(|c| c.id.as_str()).collect();
        identifiers.sort_unstable();
        let unique = identifiers.len();
        identifiers.dedup();
        assert_eq!(
            identifiers.len(),
            unique,
            "two changes share an identifier, so a diff of the diff could not tell them apart"
        );

        for change in &diff.changes {
            assert!(
                !change.reason.is_empty(),
                "a change was published with no reason"
            );
            assert!(
                change.before_value.is_some() || change.after_value.is_some(),
                "a change reports a difference with neither side recorded"
            );
        }

        assert_eq!(
            diff.summary.as_ref().map(|summary| summary.total),
            Some(diff.changes.len()),
            "a diff's summary does not count the changes it carries"
        );
    }
});
