# Default branch recovery

Leash uses `main` as its GitHub default branch. On 2026-07-10, the repository
default was corrected from the stale `feat/dotdog-spec` branch after its history
and maintained specification files were audited.

## Branch audit

Before changing the default, `main` had 25 commits that were not reachable from
`feat/dotdog-spec`. The stale branch had two commits that were not reachable
from `main`:

| Commit | Disposition |
| --- | --- |
| `c8f1900` | The original DotDog specification commit. Its complete patch is already retained on `main` as `fff993e` through PR #11. |
| `e91e769` | A merge of the then-current `main` into `feat/dotdog-spec`; it introduced no branch-side patch beyond `c8f1900`. |

`git diff origin/main...archive/feat-dotdog-spec-2026-07-10` was empty during
the audit. This proves that the stale branch had no unique patch relative to its
merge base that was missing from `main`. The direct branch tips differ because
development continued on `main` after the stale branch's last merge.

The old branch tip is preserved by the annotated tag
`archive/feat-dotdog-spec-2026-07-10`. After the tag was pushed and verified,
the remote `feat/dotdog-spec` branch was deleted.

## Repository configuration audit

- GitHub reports `main` as the default branch.
- Neither the old default nor `main` had a classic branch-protection rule, so
  the correction did not discard or bypass an existing rule.
- GitHub Actions is enabled for the repository. CI listens to all `push` and
  `pull_request` events and does not name the old branch.
- Release automation is tag- or manually-triggered and does not name the old
  branch.
- Maintained documentation, badges, and workflow files contain no stale
  `feat/dotdog-spec` references.
- New pull requests now select `main` by default. This removes the cause of the
  reverse-sync PR pattern previously visible in PR #85.

## DotDog proof

Run these checks from the repository root whenever the maintained graph changes:

```bash
npx dotdog validate .
npx dotdog compile . -o /tmp/leash.dag
cmp /tmp/leash.dag specs/leash/leash.dag
npx dotdog analyze .
```

At recovery time, validation covered all three maintained `.dog` files, compile
produced 27 nodes and 43 edges, and the generated graph matched the checked-in
artifact. Analysis found no required-file gap; it reported only the four optional
files listed by DotDog and therefore returned status 1 at 80% completeness. The
checked-in graph is generated with the locked DotDog 0.8.4 toolchain.

## Recovery procedure

If GitHub ever points at the wrong default again:

1. Compare both tips and their merge-base patch before changing repository state.
2. Account for every branch-only commit and all open reviews.
3. Validate and compile the maintained DotDog graph on the intended default.
4. Preserve any required history with a PR or annotated archive tag.
5. Change the default, verify a new PR selects it, then remove only the proven
   stale branch.
