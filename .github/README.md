# .github

This folder holds GitHub automation for Leash. Keep repository-level automation here rather than mixing CI or release behavior into runtime docs.

```mermaid
flowchart TB
  github[".github/"] --> workflows["workflows/"]
  workflows --> ci["ci.yml\nformat, clippy, tests, feature matrix, smoke-all"]
  workflows --> release["release.yml\ncrate artifact, platform binaries, draft GitHub release"]
```

## Contents

- `workflows/`: GitHub Actions workflow definitions for pull requests, pushes, tags, and manual release runs.
