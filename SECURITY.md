# Security

Please do not report a vulnerability in a public issue. Write to the maintainers instead (kevin.graff@ign.fr), with
what is affected (the engine, the SDK, a worker), how to reproduce it, and its impact. You will get an answer within a
week; a fix is released as a new version of the package, and credited if you wish.

Only the latest version of the `isard` package receives security fixes. The dependencies of the Rust crates are checked
on every push (`cargo-deny`: known vulnerabilities, licences, sources), and Dependabot keeps both workspaces up to date.
