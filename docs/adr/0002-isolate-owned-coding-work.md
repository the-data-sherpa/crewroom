# Isolate coding work from the operator's checkout

Each coding effort uses an isolated branch/worktree based on the repository's current target. The operator may explicitly select an existing branch or PR to continue after ownership checks; Crewroom does not infer ownership from whichever branch happens to be open. This adds environment provisioning work, particularly for the pilot repository's databases and browser ports, but preserves unrelated edits and makes interruption recovery attributable to the correct work.
