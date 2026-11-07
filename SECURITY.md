# Security policy

Shot Studio records the screen, camera and microphone, reads project files and runs a local MCP server. We treat capture permissions, the privacy rule for Claude, project file parsing, the MCP server and the YouTube token as security-sensitive.

## Supported versions

There is no release yet. Once the first release ships, security fixes go to the latest release only. Older releases do not get backported patches, so stay on the latest one.

## Reporting a vulnerability

**Do not open a public GitHub issue for an unpatched vulnerability.**

Report it through one of these channels, in order of preference:

1. **GitHub Security Advisories.** Use the private reporting feature: [submit a security advisory](../../security/advisories/new).
2. **Email.** Write to `security@coderscantina.com` with the subject `Shot Studio security report`.

Include as much of the following as you can:

- The Shot Studio version or commit SHA
- macOS version and Mac model
- The affected area, such as capture, the MCP server, project file parsing, the Claude integration or shipping
- Reproduction steps or a proof of concept
- What data or access is at risk
- Your CVSS 3.1 estimate, if you have one (optional)

## Response expectations

- We acknowledge a report within **72 hours**.
- We send a status update within **14 days** if a fix is not ready.
- We agree on a disclosure date with you once a fix is ready.
- We credit reporters in the advisory and release notes, if you want credit.

## What counts as a security issue

- Anything that sends raw audio, video or frames to a remote service without the user's action
- Reading or writing outside a project folder through crafted project files, `cut.kdl` or source paths
- Unauthenticated or cross-user access to the MCP server or the `shot` CLI
- Leaking the YouTube token, other secrets or private recordings through logs, the cache or a commit
- Code execution through a crafted project, media file or model download
- Model downloads that skip the pinned hash check

## What usually does not qualify

- Visual or layout bugs
- Performance problems without a plausible security impact
- Missing features
- Problems caused by a user who changed the project files on purpose

If you are not sure, report it privately first. We would rather triage a non-issue than miss a real one.
