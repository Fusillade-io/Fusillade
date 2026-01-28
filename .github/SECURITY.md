# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.7.x   | :white_check_mark: |
| < 0.7   | :x:                |

## Reporting a Vulnerability

We take security seriously. If you discover a security vulnerability in Fusillade, please report it responsibly.

### How to Report

1. **Do not** open a public GitHub issue for security vulnerabilities
2. Email us at **fusi@fusillade.io** with:
   - Description of the vulnerability
   - Steps to reproduce
   - Potential impact
   - Any suggested fixes (optional)

### What to Expect

- **Acknowledgment**: We will acknowledge receipt within 48 hours
- **Assessment**: We will assess the vulnerability and determine its severity
- **Updates**: We will keep you informed of our progress
- **Resolution**: We aim to resolve critical vulnerabilities within 7 days
- **Credit**: With your permission, we will credit you in the security advisory

### Scope

The following are in scope:
- Fusillade CLI tool
- Fusillade npm package
- Official Docker images
- fusillade.io website

### Out of Scope

- Third-party dependencies (report to upstream maintainers)
- Social engineering attacks
- Denial of service attacks

## Security Best Practices

When using Fusillade:

1. **Never commit secrets** in test scripts
2. **Use environment variables** for sensitive data
3. **Review scripts** before running them
4. **Keep Fusillade updated** to the latest version
