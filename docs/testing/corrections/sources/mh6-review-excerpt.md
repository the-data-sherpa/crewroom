## 3. P2: Runtime report retries do not replay their durable receipt

Spec Gate 2: “Same-key retries return the saved result; changed content conflicts.” MH6 contract says replay returns saved result before new transition.

`crates/crewroom-runtime/src/supervisor/workflow/choreography.rs:29-54` reads mutable files and rejects mismatched hashes before `record_package` can consult its durable receipt. If a valid report succeeds but its reply is lost and an authorized edit occurs, retrying the identical report/key fails rather than returning the saved result. Core-only replay tests bypass this runtime defect. Replay lookup/identity validation must precede current-source IO (while retaining the surrounding tool authentication).

Executed `/tmp/for11-review-replay.test.ts`: report source.txt, make a second authorized writer edit, replay exact key/payload. Got “Package artifact changed. Inspect it again before reporting.” Ledger still has exactly one saved report. Log `/tmp/for11-review-replay.log`, 1/1 confirmed case, 4.2s, cleanup passed.
