# Security policy

StateSet Freight is a developer alpha. Security fixes target the current development branch; there is no long-term support or fixed response-time commitment for earlier alpha versions.

Report suspected vulnerabilities privately to **security@stateset.io**. Do not include credentials, customer records or private shipping/payment documents in public issues or pull requests. Include the affected version or commit, impact, a minimal synthetic reproduction and any suggested mitigation. If sensitive material is required, ask for a suitable private transfer method first.

Relevant issues include tenant isolation, authentication and authorization, financial integrity, original-request recovery, encrypted terminal journals, document access and integration callback verification. Please allow coordinated investigation and remediation before publishing exploit details.

The synthetic demo is for local exploration. Production deployment requires configured access controls, database roles and migrations, identity/integration commissioning, protected recovery material and operator acceptance. See [access](docs/access.md), [deployment](docs/storage.md#separate-schema-application-from-api-startup) and [public-release readiness](docs/open-source-readiness.md).
