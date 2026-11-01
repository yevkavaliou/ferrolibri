# Security

## Reporting a problem

Please report security problems privately, not in a public issue: use **Report a vulnerability** on the repository's *Security* tab (GitHub private advisories). Say what you found, how to reproduce it and which version you used. You will get an answer as soon as the maintainer can give one; this is a small hobby project, so there is no guaranteed response time.

## What counts

FerroLibri reads a Calibre library, writes to readers, downloads files from the internet and runs third-party JavaScript plugins in a sandbox. Reports are especially welcome about:

- a plugin getting out of its sandbox, reaching a site that is not on its declared domain list, a private address, or the user's files;
- a downloaded file or a plugin result writing outside the places the app intends (path tricks in names, archives);
- the app writing to the Calibre library in any way other than Calibre's own `calibredb`;
- anything that sends or stores the user's data where it should not go.

Third-party plugins are written by others and the project does not review them; see the disclaimer in the [README](README.md#disclaimer). A malicious plugin is in scope only if it does something the sandbox is supposed to prevent.

## Supported versions

Only the latest release is supported.
