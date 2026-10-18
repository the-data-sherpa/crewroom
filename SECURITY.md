# Security

## Report a vulnerability

Do not put exploit details, credentials or private work in a public issue or pull request. Use the repository's **Security → Report a vulnerability** option when GitHub private vulnerability reporting is available.

If that option is unavailable, open an issue titled “Request a private security contact” with no vulnerability details. Wait for the maintainer to provide a private channel before sending the report. This fallback is a contact request, not a place to disclose the vulnerability.

Include the affected commit or version, platform, impact and a minimal reproduction using synthetic data. Remove tokens, account identifiers and private ticket/repository content. Do not attach your runtime directory, connection descriptor, database or model session files.

## Supported scope

Security fixes target current `main`. Historical builds and retained TypeScript rollback references have no separate maintenance guarantee. There is no guaranteed response time.

Crewroom is a local, single-operator application. The native service binds to loopback, authenticates clients and restricts request origins. Keep its data, roles and runtime configuration private. Do not expose the service through a public reverse proxy or share its bearer token.

Agents can act through configured model, repository and provider access. Use the documented execution containment and review gates. Keep final review and merge under human control. See [operations](docs/operations.md) for configuration, stopped backup/restore and recovery boundaries.
