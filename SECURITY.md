# Security

## Amasario is not a security scanner

This is the most important thing on this page.

`amasario-provenance-engine` is dependency, provenance and impact infrastructure. It
reports what was observed about a contract, where its deployed executable came from, what
evidence supports those relationships, and what a change could reach. **None of that is a
security assessment.** No output of this engine states or implies that a contract is:

- secure
- safe
- malicious
- vulnerable
- free of vulnerabilities

unless a separate, evidence-backed mechanism explicitly supports that claim - and no such
mechanism exists in this repository today. A finding of *no* adverse result means the
engine did not find one, which is not the same as there being none. The engine reports
`OBSERVED`, `VERIFIED`, `INFERRED`, `UNVERIFIED`, `CONFLICTING` and `UNKNOWN`, all of
which describe the relationship between evidence and a claim rather than the
trustworthiness of the thing the claim concerns.

`VERIFIED` deserves its own sentence. It means the evidence is consistent with the claim.
A fully verified provenance chain can describe a deliberate backdoor: the chain says the
recorded source produced the recorded artifact, and says nothing about what the source
does.

If you need a security audit, commission one. Amasario can tell you what a contract
depends on; it cannot tell you whether that is a problem.

## Reporting a vulnerability

Do not open a public issue. Use GitHub's private vulnerability reporting on this
repository (**Security** → **Report a vulnerability**), or email the maintainers at the
address on the repository profile. Include:

- what you found, and what an attacker could do with it;
- a reproduction, or as close to one as you have;
- the version (`amasario --version`), the commit, and the platform;
- whether the issue is already public anywhere.

We will acknowledge within a week and tell you what we intend to do. If you want credit in
the advisory, say so; if you want to remain anonymous, say that instead.

## What is in scope

This is a local analysis tool, and its threat model is correspondingly small. In scope:

- **A crash or panic on malformed input.** A malformed module, a hostile RPC response, a
  corrupt snapshot or a hand-edited profile must produce a classified error, not a panic
  and not an unbounded loop. The engine has fuzz targets for exactly this, and a crash
  they miss is a real bug.
- **A credential reaching an output.** Endpoint URLs embedding user information are
  rejected at construction precisely because the endpoint string is written into errors,
  reports, snapshots and logs. Any path that carries a secret into one of those is a bug.
- **A false `VERIFIED`.** The engine reporting a claim as verified when its own evidence
  contradicts it is the most damaging failure this tool can have, because a consumer acts
  on it. Same for reporting a bounded search as a complete one.
- **Arbitrary code execution.** The engine parses WebAssembly modules. It does not execute
  them, and it must not start. A crafted module that causes execution, or that causes an
  allocation the engine does not bound, is in scope.
- **A supply-chain problem in a dependency.** Report it; the dependency set is small and
  deliberately so.

## What is out of scope

- **A contract being malicious.** The engine will happily report the provenance of a
  contract that steals funds. That is the tool working correctly.
- **A remote endpoint lying.** If a node returns a fabricated instance entry, the engine
  records what the node said, qualified by the boundary it was said at. Trust in an
  endpoint is the operator's to establish; the engine's job is to not hide where the
  answer came from.
- **Denial of service against a provider.** Rate limiting is handled by bounded retries;
  using this tool to hammer someone else's endpoint is not a vulnerability report.
- **Observations of a public blockchain.** Contract addresses, deployments and dependency
  relationships on a public network are public by construction. See
  [`docs/security.md`](docs/security.md) for what the engine does and does not treat as
  sensitive.

## How the engine handles secrets

- **No secret is required to use it.** Every analysis command is read-only. There is no
  signing mode and no mutating command, so no private key is ever needed.
- **Endpoints embedding credentials are rejected.** `https://user:token@host` is refused
  when the endpoint is constructed, because that string travels into errors, reports and
  snapshots. Provider URLs that carry a key in the *path* cannot be distinguished from a
  legitimate route and are not rejected here; the guidance is to supply those through the
  environment, not a command-line argument.
- **No secret is accepted as a command-line argument.** Arguments appear in process
  history and in `ps` output. There is nothing in this tool that would need one.
- **Nothing logs a secret.** Error messages name endpoints and failure kinds, never
  credentials. No error variant in `amasario-core` has a field a credential could be
  stored in, which is a stronger guarantee than a rule about not filling one.
- **Build environments are sanitised.** `amasario-evidence`'s build record honours an
  explicit secret-name redaction list and refuses to store a value whose variable name
  looks like a secret, replacing it with a redaction marker.

## Verifying a build yourself

The engine is deterministic and reproducible by construction, and its own output is
checkable:

```console
cargo build --workspace --all-features
cargo test --workspace --all-features
AMASARIO_SPEC_DIR=.amasario-spec cargo test --workspace --all-features -- --include-ignored
scripts/run-ci.sh
```

The conformance tests are the ones that matter most: they read the normative
specification and fail when the engine's vocabulary, categories or document shapes drift
from it. If you are evaluating whether to trust this tool, read
`crates/amasario-core/tests/` and `crates/amasario-snapshot/src/` for the truncation and
contradiction handling, which is where trustworthiness is won or lost.

## Supported versions

Security fixes are applied to the default branch and released from it. Until a first
tagged release, the default branch is the supported version. Once released, the latest
minor release of the current major version is supported.
