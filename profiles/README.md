# Profiles

A profile binds the engine's generic model to one concrete ecosystem. `amasario-provenance-spec`
defines what an entity, a relationship, an evidence record and a verification status
*are*; it does not say which of them a Soroban contract on Stellar can actually produce.
A profile says that.

`stellar-soroban.yaml` is the only profile today. It records:

- **Networks.** The environments an observation can be made against, each naming a term
  from `taxonomies/network-types.yaml`, with the passphrase and network id that identify
  the chain and the endpoints that serve it.
- **Entities.** Which entity kinds this ecosystem can produce, and - the part that
  matters - which of them are observable from the chain at all. `SOURCE` and `BUILD` are
  marked unobservable, because they are. A profile that claimed otherwise would invite an
  implementation to invent a source record to make a chain look complete.
- **Relationships.** Which relationships the engine may assert here, each naming a term
  from `taxonomies/relationship-types.yaml`, with the bases that can establish it. A
  relationship whose basis is absent for a given observation is not asserted at all.
- **Guarantees and non-guarantees.** What a result from this profile means, and what it
  does not. The non-guarantees are the load-bearing half: Amasario is not a security
  scanner, a dependency is not inferred from a mention, and a verified chain says nothing
  about trustworthiness.

## What a profile is not

It is not a specification document. The specification defines no profile schema, so the
profile is engine configuration and is not validated against one. It adds no vocabulary:
every taxonomy id, relationship id and entity kind in it exists in
`amasario-provenance-spec`, and CI checks that they do against a checkout of the
specification.

It also does not define protocol behaviour. Passphrases, network ids, endpoints and which
endpoints are SDF-operated are Stellar facts. The engine's runtime source of truth for
them is `amasario-network`'s `KnownNetwork`, which derives each network id as the SHA-256
of its passphrase and asserts the derivation against the published value. The profile
records the same facts in a form a reader and a validator can inspect.

## How it is checked

`scripts/validate-profile.sh` reads this profile and a checkout of the specification, and
fails when:

- a `taxonomies/*.yaml` path named under `specification.taxonomies` does not exist;
- a relationship `id` is not a term in `taxonomies/relationship-types.yaml`;
- a network `type` is not a term in `taxonomies/network-types.yaml`;
- a network id is not the SHA-256 of the passphrase recorded beside it, which is the
  protocol rule and the check that catches a transcribed passphrase;
- a network with no `rpc` is not marked `requiresExplicitRpcEndpoint`, so that the reason
  Mainnet has none cannot be quietly lost.

`scripts/run-ci.sh` runs it. It is not yet consumed by the CLI at run time: the CLI
resolves networks through `amasario-network`, and wiring the profile into the command
surface is separate work. Until that lands, this profile is validated data and
documentation rather than something a command reads, and nothing here claims otherwise.

## Adding a profile

A new ecosystem profile is a new file in this directory plus validation rules for whatever
it asserts that the existing checks do not cover. It must reference the specification's
taxonomies rather than restating them, and it must be honest about which of the
specification's entity kinds its ecosystem cannot observe - that list is what keeps an
analysis from presenting an absence of evidence as evidence of absence.
