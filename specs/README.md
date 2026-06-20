# Specs

This folder contains DotDog planning/spec artifacts. The human-authored `.dog` files live under `specs/leash`; the compiled `.dag` is the agent-readable graph.

```mermaid
flowchart TB
  specs["specs/"] --> leash["leash/"]
  leash --> dog["*.dog\nhuman-authored project model"]
  leash --> dag["leash.dag\ncompiled graph for agents"]
  leash --> agents["AGENTS.md\nagent instructions for DotDog use"]

  dog --> compile["npx dotdog compile . -o specs/leash/leash.dag"]
  compile --> dag
  dog --> validate["npx dotdog validate ."]
```

## Folders

- `leash/`: the DotDog project for this repo.

Run DotDog commands from the repository root so project discovery sees `specs/leash`.
