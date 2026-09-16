# Pitch video — production script

A five-minute product pitch, written to be recorded in one pass rather than improvised. Every
visual named below exists in this repository or in the deployed explorer, so nothing has to be
staged except the browser.

**Not yet recorded.** When it is, this page is where the link goes, and the README gets the
thumbnail badge beside the others. Until then, the honest state is that the script exists and
the recording does not.

## The one thing the video has to land

Not "we analyse dependencies". A viewer has seen that. The claim worth five minutes is that
this tool **never presents a failure as an empty result** — that `UNVERIFIED` and `CONFLICTING`
are different words with opposite meanings, that a search which stopped at its bound says so,
and that every claim carries the basis it was made from. Everything in the script serves that
sentence, and anything that does not should be cut when the recording runs long.

## Assets to have open before recording

| Asset | Where | Used in |
| --- | --- | --- |
| Explorer overview | <https://amasario-explorer.vercel.app> | Scene 5 |
| Graph fixtures view | <https://amasario-explorer.vercel.app/#/graphs> | Scene 5 |
| Snapshot diff | <https://amasario-explorer.vercel.app/#/snapshots> | Scene 5 |
| Reference contract view | <https://amasario-explorer.vercel.app/#/contracts> | Scene 5, 6 |
| Organisation page | <https://github.com/Amasario-Soroban-Click> | Scene 4 |
| Verification status table | `docs/verification.md`, scrolled to the table | Scene 3 |
| A bounded result from a terminal | `amasario dependencies --contract CDKNJVGU2TKNJVGU2TKNJVGU2TKNJVGU2TKNJVGU2TKNJVGU2TKNJJM5 --depth 1` | Scene 3 |
| Coverage table in the README | `README.md`, `## Status` | Scene 6 |

## Voice and edit

Gemini's natural narration voice, medium pace, no rising intonation on factual sentences — the
material is already remarkable and does not need selling. Cut on sentence boundaries, not on
shots, so the visuals follow the argument. Use four transitions in total and no more: a hard
cut is better than a wipe every time the subject changes.

---

## Scene 1 — Cold open (0:00–0:22)

**Visual:** a terminal, `amasario dependencies` running against a contract id, output filling
the screen. Slow zoom on one edge with its basis beside it.

**Narration:**

> "Ask a Soroban contract what it depends on and you will get an answer. Ask how the tool knows,
> and most tools will not tell you — because the answer and the evidence live in the same field,
> and one of them is optional."

**On-screen text:** *What does this contract depend on?*

---

## Scene 2 — The problem (0:22–0:55)

**Visual:** two JSON snippets side by side. Both show `"dependencies": []`. Label one *the
search found nothing* and the other *the search did not run*.

**Narration:**

> "Here are two results from a real tool. Both say: no dependencies. One of them means the
> analysis looked and found none. The other means the endpoint refused the connection and the
> tool carried on. In a security review, in a release gate, in an incident, those two sentences
> are not the same — and in most output they are the same bytes."

> "Provenance has the same shape. A contract's build history is either *asserted* or *evidenced*,
> and a tool that cannot tell you which is doing neither."

---

## Scene 3 — The claim (0:55–1:45)

**Visual:** `docs/verification.md`, the status table. Zoom the `UNVERIFIED` and `CONFLICTING`
rows as they are read. Then the terminal, the depth-bounded command, and the truncation line in
its output.

**Narration:**

> "Amasario is read-only infrastructure for provenance, dependency and impact analysis on
> Soroban. It has one governing rule that shapes every layer: it must never convert a failure
> into an empty result."

> "So verification has five statuses, and they are not a scale. *Verified* means the evidence is
> consistent with the claim. *Unverified* means nothing was obtained to check against.
> *Conflicting* means the evidence refutes it, or two sources disagree. Collapsing those would
> make not being able to check read as a failure to check — which is the opposite fact."

> "A bounded search reports its bound. That output says the traversal stopped at depth one, so a
> short list is never mistaken for a complete one. And every relationship carries its basis:
> which observation, which rule, which window it was observed in."

**On-screen text:** *A failure is never an empty result.*

---

## Scene 4 — Architecture (1:45–2:25)

**Visual:** the organisation page, then a simple four-box diagram — normative, execution,
presentation, cross-cutting — with the arrows labelled *validated against*, not *imports*.

**Narration:**

> "It is four repositories, and the boundaries between them are the design."

> "The specification is normative: schemas, taxonomies and deterministic vectors that define what
> a document *means*. It ships no implementation, because a specification with one
> implementation is that implementation's documentation — and because a change to meaning should
> fail as a schema error rather than as a quietly different analysis result."

> "The engine is this repository: twelve crates that observe, classify and refuse. The explorer
> renders the engine's own committed documents, and analyses nothing — every file's digest is
> re-checked in its own CI. And the documentation repository holds the pages that span the
> seams: policy, compatibility, and the gaps."

---

## Scene 5 — It running (2:25–3:25)

**Visual:** the explorer, live. Overview, then the graphs view with a real fixture, then the
snapshot diff, then the reference contract's decoded modules. Move between views at a readable
pace; let each view sit for two seconds before a slow zoom onto the data table.

**Narration:**

> "This is the deployed explorer, reading documents this engine produced. A dependency graph,
> with every edge traceable to the observation that supports it."

> "A snapshot comparison — what changed between two captures of one contract's state. A change
> that did not happen cannot appear here, because the digest is computed over a normalised
> state rather than over raw response ordering."

> "And the reference contract: a caller and a callee built from source in the project, whose
> compiled bytes are committed with their digests and rebuilt in CI to prove the build is
> reproducible. It is here to be analysed, not deployed."

> "Notice what the explorer does not do. It does not fetch, it does not parse WebAssembly, and it
> does not compute a single value. It is a viewer, deliberately, so the picture cannot claim more
> than the table beside it."

---

## Scene 6 — Engineering depth (3:25–4:10)

**Visual:** the README status section, scrolling through the test counts and the coverage table.
Then the GitHub Actions list, then the issue list.

**Narration:**

> "The engineering is meant to be checkable rather than asserted. Eleven hundred and two tests
> across twelve crates and eleven end-to-end suites, at eighty-eight percent line coverage, with
> the two caveated figures published rather than rounded away."

> "Five fuzz targets, one of which found a real defect: a bounded path search that could return
> one more path than its own bound, now fixed and regression-tested from the fuzzer's artefact."

> "Ten CI workflows, including a scheduled live-network run and a reproducibility check that
> rebuilds the reference contract and diffs the bytes. And fifty-six open issues spread across
> the layers — refusals, bounds, formats, the harness, the explorer, the specification — because
> the contribution surface is the point."

---

## Scene 7 — Value and close (4:10–5:00)

**Visual:** the explorer overview with the organisation page in a second pane. End on the
repository list.

**Narration:**

> "What this is worth depends on what you need from it. If you are reviewing a contract before
> you integrate it, it shows you what it depends on and how strongly that is known. If you are
> gating a release, it gives you a document your pipeline can branch on — including on *unknown*.
> If you are building on Soroban and want to implement this vocabulary rather than consume it,
> the specification is normative, versioned and has a conformance suite rather than a reference
> implementation you have to read."

> "Amasario is Apache-2.0, the explorer is live, and the issues are written to be picked up
> rather than admired. The claim it makes is small and it holds everywhere: it reports what it
> observed, it names the boundary where observation stopped, and it never turns a failure into
> silence."

**On-screen text:** *Amasario — provenance, dependency and impact infrastructure for Soroban.*
Then the organisation URL and the live explorer URL.

---

## Recording checklist

- [ ] Record at 1920×1080, 60 fps, with the terminal at a legible font size for a phone screen.
- [ ] Hide the browser bookmarks bar and any personal tab.
- [ ] Confirm every URL shown resolves on a machine that is not yours, before publishing.
- [ ] Generate the voice-over with Gemini, listen once for a mispronounced project name, and
      re-cut that sentence rather than re-recording the scene.
- [ ] Export under 1080p / under 500 MB, so Loom transcodes cleanly.
- [ ] Upload to Loom with the title, description and the explorer link, and set it to public.
- [ ] Put the Loom link in `docs/pitch-video.md` *and* the README, and add the thumbnail badge
      beside the existing badges.
- [ ] Open the published link in a private window, to prove it is public rather than
      org-visible — a video nobody outside the organisation can watch is worse than no video,
      because the badge implies otherwise.
