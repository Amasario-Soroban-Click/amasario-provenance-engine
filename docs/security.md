# Security boundaries

Amasario is dependency, provenance and impact infrastructure. **It is not a security
scanner, and nothing it reports is a security assessment.**

## What the engine will not claim

The engine does not report, and does not imply, that a contract is:

* secure;
* safe;
* malicious;
* vulnerable;
* free of vulnerabilities.

Those are claims about behaviour and intent, and the engine reads ledger state. It cannot
observe either.

What it reports instead are factual states:

| State | Meaning |
| --- | --- |
| `OBSERVED` | The endpoint returned this at the boundary. |
| `VERIFIED` | The evidence is consistent with the claim it is attached to. |
| `INFERRED` | It follows from observations by a stated basis. |
| `UNVERIFIED` | Nothing supports the claim. |
| `CONFLICTING` | The evidence refutes the claim. |
| `UNKNOWN` | The question could not be asked. |

`VERIFIED` states that the evidence is consistent with a claim. It states nothing about the
trustworthiness of the entity the claim concerns. **A fully verified provenance chain can
describe a deliberately backdoored contract.** Every report the engine writes carries that
sentence, in the report document itself rather than only in documentation, so a consumer
that reads the JSON meets it whether or not they read this page.

## Relationship to a real audit

| An audit asks | Amasario asks |
| --- | --- |
| Is this contract safe to hold funds? | What does it depend on? |
| Can this be exploited? | Where did this executable come from, and how well is that established? |
| Is this logic correct? | What does the evidence support, and what does it leave open? |

The second column is useful input to the first. It is not a substitute for it, and a
`VERIFIED` result is not a clean bill of health. See [verification.md](verification.md).

## Credentials

The engine takes no credential.

* There is no API key, no mnemonic and no private key in its interface.
* Every command is read-only. Nothing it does mutates chain state.
* `--passphrase` is not a secret: a network's passphrase is published, and its purpose is
  to check that the endpoint really serves the chain the caller named.
* Secrets are never accepted through command-line arguments, because command-line arguments
  appear in process history. There is nothing to pass that way.
* The engine logs no secret. It has none to log.

If you need an authenticated endpoint, put it behind a proxy rather than putting a
credential on the command line; the engine will still refuse to retry a rejected request,
which is the behaviour you want from a proxy-fronted endpoint.

## Untrusted input

Three classes of input are not the engine's own, and each is handled as untrusted.

| Input | Handling |
| --- | --- |
| Network responses | Classified: an absence, a transient failure, a permanent failure or a malformed response. Never silently converted into an empty result. |
| Module bytes | Decoded total-function style. Any byte string is either a module or a refusal, never a panic. The digest is recomputed rather than trusted. |
| Stored snapshots | Parsed with `deny_unknown_fields`, validated, and checked against their content digest before anything is compared. |

The fuzz targets in `fuzz/` exist for exactly these paths, and their assertions are the
specification's rules rather than only "did not crash" — see
[ci-integration.md](ci-integration.md).

## What is deliberately not available

| Feature | Why not |
| --- | --- |
| Writing to the chain | The engine is an analysis tool. Mutating state is out of scope, so a compromised engine cannot move funds. |
| Enumerating or scanning a network | The engine observes the contract it is asked about. It is not a block explorer and does not crawl. |
| Following a dependency a caller did not ask about | Traversal is bounded by the caller's bounds, and the bound used is recorded. |
| Unbounded traversal | `MAX_PERMITTED_DEPTH` is 32 and the node bound defaults to 10000. An unbounded traversal is prohibited rather than defaulted away. |
| Assuming a source revision from a contract address | Source identity and deployed identity are separate claims and are compared, not inferred from each other. |

## Privacy

Provenance and dependency analysis reveals relationships between projects and deployment
history. A snapshot is a record of what a contract depends on, when it was deployed and
where its source lives, and it can expose operational detail a project has not published.

Responsible use:

* **The engine requires no private source code.** Source provenance is established from a
  repository URL and a revision — both usually public — and from digests. Reading a private
  repository is not needed and is not done.
* **The engine requires no secrets**, as above.
* **A snapshot is data about someone else's project.** Treat an exported analysis the way
  you would treat a list of a dependency tree: fine to publish for an open-source
  contract, and worth a thought before publishing for anything else.
* **GitHub source anchoring in the specification is opt-in.** Where a profile supports it,
  anchoring a repository to a revision is a deliberate step rather than a default.

`--horizon` is what enables deployment history, and deployment history is what identifies
the transaction that deployed a contract. Running without it produces a chain whose
deployment link is `UNVERIFIED`, which is a smaller disclosure and a weaker analysis. Both
are legitimate; the choice is the caller's and the report says which was made.

## Reporting a vulnerability

See [SECURITY.md](../SECURITY.md) in the repository root for how to report a defect in the
engine itself, and what is in scope.
