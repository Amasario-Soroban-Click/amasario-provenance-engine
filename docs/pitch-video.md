# Pitch video — production script

A five-minute product pitch, written to be recorded in one pass rather than improvised. Every
visual named below exists in this repository or in the deployed explorer, so nothing has to be
staged except the browser.

**Recorded.** Watch it at
[amasario-explorer.vercel.app/pitch/amasario-pitch-v1.mp4](https://amasario-explorer.vercel.app/pitch/amasario-pitch-v1.mp4)
— five minutes, 1080p, narrated. The file is served by the explorer's deployment rather than
by a video host, so the link does not depend on a third-party account staying open, and the
README carries the thumbnail badge beside the others.

This page remains the script. Where the recording and the script disagree, the recording is
what was said and this page is what was intended; where either disagrees with the documents
this repository produces, the documents are what the project claims.

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

**Superseded after the recording was made.** The last sentence was true when the video was
rendered and is not true now: both halves are deployed to Testnet, with the deployed modules
hashing to the committed fixtures, and [testnet.md](testnet.md) records the contract IDs and
the transaction in which the caller entered the callee.

The audio is left as recorded rather than re-cut, and the trade is stated rather than hidden.
Re-cutting means re-recording the narration line, which shifts every later shot's timing,
because shot durations are derived from the narration - so the cost is a full re-render rather
than an edit. What a viewer of the recording alone is misled about is one clause: that the
fixture is a fixture. What they are *not* misled about is the claim the clause exists to
support, which is that the bytes are committed and that CI rebuilds them and compares - both
still true, and both what the sentence is doing the work of.

Being explicit about the direction of the correction: this page is the script, the recording
is what was said, and the documents are what the project claims. Where the recording is out
of date with the documents, the documents are right - which is this paragraph's whole point.
A re-render is the only thing that removes the discrepancy, and the render is reproducible
from this page, so it is a matter of running it again rather than of recovering anything.

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

## How it was recorded

Not with a screen recorder. Every frame is a 1920×1080 composition built around a live
capture of the deployed explorer or the organisation's pages, with callouts drawn at the
measured bounding box of the element they name — so a label cannot end up pointing at the
wrong row. Shot durations are derived from the narration rather than chosen, so a sentence is
never cut off by the next cut, and the whole thing re-renders from committed inputs.

That makes the deviations from the plan above worth putting on the record rather than
quietly dropping:

- **30 fps, not 60.** The picture is a slow move over near-static frames. The extra frames
  bought nothing a viewer can see and cost a full re-encode of every shot.
- **Piper, not Gemini.** The narration is a local neural voice. A Gemini voice-over needs an
  API key this environment did not have. The words are unchanged and the voice is a single
  argument to the build, so re-rendering in another voice is one line.
- **Served by the explorer, not Loom.** The file lives in
  `amasario-explorer/public/pitch/` and is served by that deployment, so the video is
  versioned with the code it describes and its link cannot lapse with an account.

Two properties are checked rather than asserted. Every shot is confirmed to be on screen at
its expected time by decoding a frame and comparing it against the image that shot was built
from, and every narration clip is confirmed to begin within a tenth of a second of where the
timeline places it. Both checks are in the build described below.
