<!--
Thanks for the pull request. The three sections below are the ones a reviewer needs;
the checkboxes are the checks CI runs anyway, listed so you know before pushing rather
than after.

If this change alters what the engine *means* - a relationship, an evidence class, a
confidence level - it belongs in amasario-provenance-spec first. See CONTRIBUTING.md.
-->

## What changed, and why

<!--
The "what" is in the diff. The "why" is what a reader six months from now needs, and it
is the part a reviewer cannot reconstruct. If this fixes a bug, say what the incorrect
behaviour was and how it could have been observed.
-->

## How this was verified

<!--
The commands you ran and what they printed. "Tests pass" without the command is not
verification. If you could not verify something, say which part and why - an
acknowledged gap is a gap; an unacknowledged one becomes a bug report.
-->

```console
$ scripts/run-ci.sh
```

## What this does not do

<!--
Known gaps, deferred cases, and anything a reviewer might reasonably assume is included
and is not. Being explicit here is how a limitation stays a limitation.
-->

## Checklist

- [ ] `scripts/run-ci.sh` passes locally.
- [ ] Each commit is one coherent change, with a message that explains the "why".
- [ ] New behaviour has tests, and the tests are named as the property they assert.
- [ ] A claim the engine produces carries a basis and at least one evidence citation.
- [ ] A code path that can fail in a way a caller must distinguish is classified, not
      collapsed into one error.
- [ ] A bounded result says it was bounded, and with what reason.
- [ ] Two runs over the same input, boundary and engine version produce identical output.
- [ ] Documentation is updated, and no documented command or flag is one the binary does
      not have.
- [ ] No secret, credential, private key or real account address is in the diff or in a
      fixture.
- [ ] If a dependency was added, the reason is stated in the pull request and in the
      workspace manifest.

## Related issues

<!-- Fixes #123, or "none". -->
