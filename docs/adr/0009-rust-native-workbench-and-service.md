# Port the workbench and workflow service to Rust

On 2026-10-02 the operator selected Rust with GPUI Kit and explicitly included the workflow service in the port. Keep a separate daemon as the sole workflow authority, with the native app as an authenticated local client, so closing the UI cannot stop or take ownership of execution. Preserve the SQLite records and existing supervision, containment, message, and revision-evidence contracts; cut over from the validated TypeScript implementation only after the affected contracts pass in Rust.

The [native migration plan](../native-migration.md) tracks implementation and remaining acceptance work. This decision authorizes the full port; it does not declare native feature parity or transfer the retained live pilot to the Rust daemon.
