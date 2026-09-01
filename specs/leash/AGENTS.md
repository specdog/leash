# AGENTS.md — leash DotDog project

> Agent instructions for querying the Leash project graph.

## Rule: query the compiled graph

Do not read or edit `.dog` files directly as an agent. `.dog` is the human-authored format; `leash.dag` is the compiled agent format.

- `.md`: read for implementation and contributor context.
- `.dog`: human-owned source; do not infer project structure from it.
- `.dag`: query through DotDog MCP for project entities, relationships, and intended states.
- If the `.dag` does not contain a fact: report it as unverified instead of falling back to `.dog`.

Start the local project graph server from the repository root:

```bash
npm ci
npx dotdog serve
```

Use the DotDog MCP tools (`getEntity`, `traverse`, `search`, `schema`, `summary`, `listProjects`) rather than parsing the DAG manually when MCP is available.

## Current repository shape

```text
leash/
  AGENTS.md              root coding-agent instructions
  CONTRIBUTING.md        human open-source workflow
  crates/
    leash-core/          reusable contracts/core types
    leash-runtime/       runtime orchestration
    leash-cuda/          bounded advisory CUDA compute
    leash-gateway/       gateway boundary
    leash-replay/        replay support
    leash-ros2/          ROS 2 provider/proposal boundary
    leash-waveshare/     reusable Waveshare adapter code
  src/                   top-level CLI/HTTP/MCP integration
  implementations/
    waveshare-ugv/       concrete deployment, calibration, field proof
  specs/leash/
    SPEC.dog             human project overview/stories
    constitution.dog     human safety/feature rules
    data-model.dog       human entity/relationship source
    leash.dag            compiled project graph for agents
    AGENTS.md            this file
```

The Rust workspace is implemented and active. Do not treat `crates/` as planned or empty.

## Implementation vs. project graph

For runtime behavior, current code/tests/schemas on `main` are authoritative. For intended project structure and relationships, query the DAG. If implementation and DAG disagree, report the drift explicitly so the human-owned `.dog` source can be corrected and recompiled.

Never use a stale draft PR as shipped state. Open PRs may describe in-progress behavior that is not on `main`.

## Safety boundary

Leash remains the sole final writer to physical devices. Agents, ROS 2, planners, localization, mapping, model providers, and CUDA compute may request actions or provide evidence, but they do not bypass capability policy, authorization, approval, freshness, deadman, collision checks, stop, or E-stop.

Simulation/replay must remain non-actuating. Do not claim physical proof without real hardware evidence.

## Verification

For implementation changes, follow the root `AGENTS.md` and `.agents/skills/verify/SKILL.md`.

Humans changing the spec should validate and regenerate the compiled graph:

```bash
npx dotdog validate .
npx dotdog compile . -o /tmp/leash.dag
cmp /tmp/leash.dag specs/leash/leash.dag
npx dotdog analyze .
```
