# Snapshots

A snapshot is one analysis, at one boundary, on one chain, written down. It is the artefact
that makes two runs comparable: a release pipeline captures before and after, and the diff
is the finding.

```bash
amasario snapshot create --contract CAAAA...D2KM --network testnet -o snapshot-a.json
amasario snapshot show --input snapshot-a.json --format json --pretty
amasario diff --before snapshot-a.json --after snapshot-b.json --format markdown
```

## What a snapshot holds

| Section | Contents |
| --- | --- |
| Contract identity | Address, network, module digest, the ledger observed at, first and last observed ledger. |
| WASM identity | The module digest, its type, its size and the evidence that establishes it. |
| Provenance | The chain, link by link, with each link's status and confidence. |
| Dependencies | The dependency set: the direct and transitive partitions, the refusals and the cycles. |
| Graph | The graph document the dependencies project onto. |
| Evidence | Every record any claim in the snapshot cites. |
| Confidence | The snapshot's overall confidence, with its citations. |
| Impact | The findings the analysis produced. |
| Attestations | The external statements that were collected. |
| Boundary | The network, the ledger, the observation time and the specification version. |
| Versions | The specification version and the engine version that produced it. |

## Canonicalisation and the content digest

A snapshot carries a `contentDigest`: a digest over its canonical form, with the declared
volatile fields removed.

**Canonicalisation** sorts every collection by an explicit key — evidence and attestations
and findings by identifier, the dependency partitions by their own key, graph nodes and
edges by theirs. Two snapshots that differ only in the order their collections arrived in
are the same snapshot, which is what makes a comparison meaningful. The `snapshots`
integration suite asserts this by comparing a snapshot with a reversed copy of itself and
requiring no differences.

**The volatile fields** are the ones that legitimately differ between two runs of one
analysis — the capture time, the engine version where it is not part of the model, and
anything else that describes the run rather than the contract. They are listed in
`DEFAULT_VOLATILE_FIELDS`, and `validate_volatile_fields` refuses a digest computed with a
field that is not declared volatile. That is the check that stops a digest from quietly
excluding a field it should have covered.

**`verify_content_digest`** recomputes the digest and compares. A snapshot whose recorded
digest disagrees with its contents is refused before any comparison happens, because every
diff rests on the two documents being what they claim to be.

## Comparison

```bash
amasario diff --before a.json --after b.json
```

The categories a difference can fall into:

| Category | Meaning |
| --- | --- |
| `CONTRACT_IDENTITY_CHANGED` | A change to the contract's own identity. |
| `WASM_IDENTITY_CHANGED` | A changed module digest. |
| `PROVENANCE_CHANGED` | A provenance link that changed. |
| `DEPLOYMENT_CHANGED` | A changed deployment. |
| `EVIDENCE_CHANGED` | An evidence record added, removed or altered. |
| `RELATIONSHIP_ADDED` | An edge the later capture asserts and the earlier one did not. |
| `RELATIONSHIP_REMOVED` | An edge the earlier capture asserted and the later one does not. |
| `RELATIONSHIP_CHANGED` | An edge present in both whose confidence, verification or classification moved. |
| `CONFIDENCE_CHANGED` | A change to how strong the evidence is. |
| `IMPACT_SURFACE_CHANGED` | A finding added or removed, so what a change could reach has moved. |
| `TRUNCATION_CHANGED` | Whether the analysis was bounded changed, which changes what the rest of the diff means. |

The categories are reported in a fixed order — identity, then the chain from source to
deployment, then the relationships derived from it, then the analysis over those — because
that is the order a reader forms the picture in, and fixing it is what makes two runs of
one comparison write byte-identical documents.

A relationship present in both whose confidence changed is reported as **changed**, not as
an addition, so a consumer filtering for new dependencies does not see a strengthened edge
as a new one.

## Change identifiers

A change's identifier is derived from its category, its entity and its path, and the path
names the *element* rather than the collection: `/impact/<finding-id>`,
`/evidence/<record-id>`, `/dependencies/<triple>`. Path segments are escaped as RFC 6901
requires, so an identifier containing a slash cannot invent a level of nesting.

This matters more than it looks. Two findings added between two captures are two changes,
and reporting both at a bare `/impact` would give them one identifier — which the diff's
own duplicate rule then refuses, leaving no diff at all. Stable per-change identifiers are
also what let a diff of a diff be meaningful.

Every entry carries a reason and a change type, and one that states no reason is refused by
the diff that carries it. A diff is the artefact most likely to be consumed without review,
so a reviewer needs something to reject an entry on.

## Comparability

Two snapshots are comparable when they describe the same contract on the same network at
compatible boundaries. When they are not, the diff reports the pair as incomparable with
the reason rather than comparing them anyway:

```json
{
  "comparable": false,
  "incomparableReason": "..."
}
```

Comparing a contract with a different contract, or one contract on two chains, would
produce a list of differences that are really just the two subjects being different.

## Reading a snapshot back

`amasario snapshot show` reads a stored snapshot and reports what it holds, which is also
the check that a snapshot written by an earlier engine is still readable. A document that
does not parse, or that parses but disagrees with a requirement, is refused with the first
disagreement named — never partially accepted.

## Fixtures

`fixtures/snapshots/` holds one pair, `testnet-alpha-before.json` and
`testnet-alpha-after.json`, captured at one boundary on one contract and differing so that
a diff has an added relationship, a changed module digest and a changed confidence to
report. The file names are what the workflows glob for when they exercise `amasario diff`.
