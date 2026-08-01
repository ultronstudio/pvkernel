# Security Policy

## Project Status

pvKernel is currently an experimental, early-stage operating system kernel.

It is not suitable for production use, security-critical systems, or systems
containing sensitive data. Security boundaries, memory protection, userspace
isolation, permissions, and other hardening mechanisms are still under
development.

Security reports are nevertheless welcome, especially when they concern code
that is already implemented.

## Supported Versions

Only the latest release and the current `main` branch receive security fixes.

| Version | Supported |
| ------- | --------- |
| `main` | ✅ |
| Latest release | ✅ |
| Older releases | ❌ |

Security fixes are generally not backported to older development releases.

## Reporting a Vulnerability

Please do **not** report security vulnerabilities through public GitHub issues,
pull requests, discussions, or other public channels.

Use one of the following private reporting methods:

1. Submit a private vulnerability report through GitHub:
   https://github.com/ultronstudio/pvkernel/security/advisories/new

2. If private vulnerability reporting is unavailable, contact:
   contact@petrvurm.cz

Please include as much of the following information as possible:

- a clear description of the vulnerability;
- the affected pvKernel version or commit;
- steps required to reproduce the issue;
- a minimal proof of concept, if available;
- the expected and actual behavior;
- the potential security impact;
- any suggested mitigation or fix;
- whether the issue has already been disclosed elsewhere.

Do not include secrets, personal data, or information belonging to third
parties unless it is strictly necessary for understanding the report.

## Response Process

I aim to acknowledge a security report within 7 calendar days.

After reviewing the report, I will indicate whether it has been:

- accepted as a security vulnerability;
- accepted as a regular bug;
- declined because it cannot be reproduced;
- declined because the affected functionality has not yet been implemented;
- declined because it is outside the scope of pvKernel.

For accepted vulnerabilities, discussion should remain private until a fix or
mitigation is available. Updates will be provided when meaningful progress is
made, but no fixed remediation deadline can currently be guaranteed.

When appropriate, a fix may be published together with:

- a patched release;
- a GitHub Security Advisory;
- a description of affected versions;
- mitigation or upgrade instructions;
- acknowledgement of the reporter.

Reporters will be credited unless they request anonymity.

## Scope

Examples of issues that may qualify as security vulnerabilities include:

- memory corruption in implemented kernel functionality;
- privilege boundary violations;
- incorrect memory mapping or access control;
- unsafe handling of untrusted input;
- vulnerabilities in parsers, loaders, filesystems, or network code;
- vulnerabilities that allow unintended code execution;
- insecure build or release processes;
- compromised or malicious dependencies;
- exposure of secrets or sensitive development infrastructure.

The following are generally not treated as security vulnerabilities during
the current early development stage:

- missing security features that are explicitly documented as unimplemented;
- crashes caused only by unsupported hardware or configurations;
- denial of service caused by unfinished functionality;
- theoretical issues without a practical impact on implemented code;
- vulnerabilities that exist only after modifying pvKernel's source code;
- general feature requests or code-quality suggestions.

If you are unsure whether an issue is security-related, report it privately.

## Coordinated Disclosure

Please allow reasonable time to investigate and prepare a fix before publicly
disclosing a vulnerability.

Do not exploit a vulnerability beyond what is necessary to demonstrate it,
access data belonging to other people, disrupt third-party systems, or perform
testing against systems without authorization.

## Bug Bounty

pvKernel does not currently operate a paid bug bounty program.

Submitting a report does not create an entitlement to payment or other
compensation.