# AGENTS.md — leash

> Rust harness runtime for robot control. MCP + CLI + HTTP.

## Quick Start

**NEVER read .dog files directly.** Query the .dag via MCP. The .dog is human format. The .dag is agent format.

- **.md**: read for context (README, docs) but NOT for entity/relationship/project structure
- **.dog**: NEVER read. Human writes it. Agent never touches it.
- **.dag**: ALWAYS query via MCP. This is your source of truth for project structure.
- **If .dag doesn't have it**: report "unverified" — never fall back to .dog

**MCP**: `npx dotdog serve` (6 tools: getEntity, traverse, search, schema, summary, listProjects)

## Project

```
leash/
  specs/leash/          dotdog specs
    SPEC.dog            Project overview + user stories
    constitution.dog    Safety + feature-gating rules
    data-model.dog      human-authored entity + relationship source
    leash.dag           Compiled (27 nodes, 42 edges; agent reads this)
  crates/               Rust workspace (planned)
```

## Entities

| Entity | States | Description |
|--------|--------|-------------|
| Harness | planned→mapped→extracted→stabilized→released | Core runtime |
| CLI | planned→implemented→tested→released | CLI + HTTP + MCP HTTP server |
| MCPServer | planned→implemented→tested→released | MCP stdio and localhost MCP HTTP for LLM agents |
| UGVAdapter | planned→implemented→feature_gated→documented | Waveshare UGV |
| Bridge | planned→mapped→documented→tested | Robot bridge compat |
| Safety | planned→implemented→verified | Smoke tests + gates |
| ReleasePipeline | planned→configured→publishing | crates.io + binaries |
| Bootstrap | planned→bootstrapped→ci_green | Crate skeleton + CI |
