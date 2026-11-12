# Security policy

routwatch holds a router password and can write to the router, so reports about
credential handling and about writes are taken seriously.

## Supported versions

| Version | Supported          |
| ------- | ------------------ |
| 0.5.x   | yes                |
| < 0.5   | no, please upgrade |

## Automated checks

- Dependabot alerts and security updates are on, and `.github/dependabot.yml` opens
  a weekly pull request for `cargo` and for the GitHub Actions it pins.
- CodeQL runs on every push to `main` and every pull request (default setup, Rust
  and Actions query packs). Results land in **Security -> Code scanning**.
- Secret scanning and push protection are on: a commit that carries a token or a
  credential is refused at push time.

## Reporting a vulnerability

Please do not open a public issue. Use GitHub's private reporting on this
repository: **Security -> Report a vulnerability**. It opens a private advisory
that only the maintainer can read, and it is the only supported channel.

Include the routwatch version (`routwatch --version`), the router model and
firmware, and what you saw happen. Do not attach a real config file, or a capture
containing your SSID, MAC, IMEI, serial or WAN IP - redact first.

What to expect: an acknowledgement, then an assessment. If the report is
confirmed, the fix goes into the next patch release, and you are credited in the
release notes unless you ask not to be.

## In scope

- Anything that leaks the router password or the config file's contents - to the
  terminal, to `--json`, to a report file, or to a log line.
- Config file permissions. The file that holds the password is written `0600`;
  anything that weakens that, or a write that is not followed by a read-back
  check, is a bug.
- A write that reports success after the router refused it.
- A crafted or broken router response (truncated XML, an absurdly long field, an
  unexpected redirect) that panics the binary or triggers a write the user never
  confirmed.
- Command injection or path traversal reached through a value in the config.

## Not a security issue

- `--json` and the `full` report mode printing your own identity or WAN IP - that
  is what asking for them means. The default report is `redacted`.
- Another process running as the same user reading your config file. It holds your
  router credential on purpose, protected by `0600` and nothing else.
