# Evidence

Evidence is what turns a claim into something a reader can check. Every dependency edge,
every provenance link, every finding and every confidence carries at least one citation to
an evidence record, and the record says what was observed and when.

```bash
amasario dependencies --contract CAAAA...D2KM --network testnet --format json --pretty
```

## The record

An evidence record is one of nine classes, and the class decides what the record must
carry.

| Class | Records | Required |
| --- | --- | --- |
| `SOURCE` | A repository and a revision. | The repository URL and the revision. |
| `BUILD` | A build and what it produced. | The build's source revision and its artifact digest. |
| `ARTIFACT` | A produced artifact. | Its digest and its type. |
| `WASM` | A module and its identity. | The module digest. |
| `DEPLOYMENT` | A deployment and what it recorded. | The deployment transaction and the module digest. |
| `TRANSACTION` | A ledger transaction. | The transaction hash, its ledger, and whether it succeeded. |
| `EVENT` | A contract event. | The event and the contract that emitted it. |
| `ATTESTATION` | An external statement about a build. | The issuer and what was attested. |
| `OBSERVATION` | Something seen at a boundary. | The note describing what was seen. |

A record's class is not decoration: `validate()` refuses a record whose class-required
fields are missing, and the `evidence` suite asserts that over the corpus. A `TRANSACTION`
record with no `successful` flag is refused rather than treated as an unknown outcome,
because an unknown outcome is not a successful one and the difference is what the runtime
dependency rule rests on.

## Citations, not copies

A claim cites a record by class and identifier — `TRANSACTION:0101...0101` — rather than
embedding it. That is what makes a document self-consistent: the same transaction cited by
two edges is one record, and a diff can tell that an edge changed while its evidence did
not.

`supportsRelationships` on a record names the relationships a record bears on. It is
omitted when empty, because the schema refuses an empty array where the property is
optional, and it is read back as empty when omitted — a producer that writes an omission
must accept the omission.

## What the classes are kept apart for

The engine never promotes one class to another.

* An **event** is not a transaction. A cross-contract call observed in an event is
  evidence, and its basis is `OBSERVED_EVENT`, which is a different basis from
  `OBSERVED_INVOCATION` even though both reach `VERIFIED` confidence. A reader can tell
  which was found.
* An **attestation** is not a computation. It says an issuer claimed something; it does
  not make the digest agree with the bytes. The engine still recomputes.
* An **observation** is not a fact about behaviour. It records that something was present
  at a boundary — an instance entry, a storage value — without inferring anything from it.

## Collection

The collector reads within the observation boundary and attributes what it finds to the
contract that produced it. A cross-contract call is attributed to its caller, which is why
a dependency set holds only the subject's own observations and why a second hop comes from
a different contract's set. See [dependency-analysis.md](dependency-analysis.md).

Bounds that apply:

| Bound | Default | Effect |
| --- | --- | --- |
| `--depth` | 6 | How far traversal may go before it must say it stopped. |
| `--max-nodes` | 10000 | How many entities traversal may accumulate. |
| `--max-transactions` | 16 | How many transactions are read for invocation evidence. |
| `--scan-events` | off | Whether events are read, which is what finds cross-call invocations. |

## Verification of evidence

An evidence record is checked before it is used to support anything. The verifier asks
whether the record is internally consistent and whether the claim it is attached to is the
thing it describes. A `TRANSACTION` record cited for an invocation must be the
transaction the invocation was recorded in; a `WASM` record cited for a digest must carry
the digest that was recomputed.

## Fixtures

The corpus's evidence records are built by `documents::evidence()` in the
`amasario-integration-tests` crate and are asserted by the `verification` suite, which
checks that every record satisfies its own class and that every citation resolves. Nothing
in the corpus is transcribed from a live chain: the records are *shaped* like the
documented responses they stand in for, and the adapters' own parsers are what decode the
recorded bodies.

## Confidence rests on evidence, never replaces it

A confidence carries the citations it rests on, and a confidence with no citation cannot
be constructed. The level is capped by the basis. See
[dependency-analysis.md](dependency-analysis.md) for the ceiling table and
[verification.md](verification.md) for how status and confidence differ.

## What evidence is not

Evidence is not proof of intent or of safety. It is a record that something was observed,
at a boundary, by a named mechanism. A claim supported by excellent evidence can still be
about a malicious contract. See [security.md](security.md).
