<!--
Keep this short. The two sections that reviewers actually rely on are the
verification output and the compatibility checklist — the rest is context.
-->

## What changes

<!-- The user-visible change, in a sentence or two. Why, if it is not obvious. -->

Fixes #

## Verification

<!--
Paste the result, not just the command. `scripts/check.sh` is the canonical gate
and CI runs exactly it, so a green run here is the strongest thing you can show.
If you only ran a subset, say which — a reviewer cannot tell the difference
between "the rest passed" and "the rest was not run".
-->

```
$ scripts/check.sh
```

## Compatibility

<!-- Delete any line that does not apply; keep the ones you can affirm. -->

- [ ] No golden fixture under `compat-fixtures/fixtures/` or `tui/testdata/` changed.
      *(If one did: explain below what on-disk or on-wire format moved, and why
      that is safe. Two TUI goldens embed a width-truncated path, so even a
      string-length change can move them.)*
- [ ] No TypeScript-CLI compatibility name was renamed (`CLAUDE_CONFIG_DIR`,
      `ANTHROPIC_API_KEY`, `~/.claude`, `settings.json`, ...). `scripts/audit-brand.sh`
      passes.
- [ ] New public API is intentional — `scripts/audit-public-surface.sh` passes, and
      `public-api-allow-list.txt` was updated deliberately if it needed to change.
- [ ] Behavioural change is covered by a test in the affected crate.

## Notes for the reviewer

<!--
Anything worth flagging: a tradeoff you took, something you chose not to do, a
follow-up you would like tracked. For a TUI behaviour change, a before/after
terminal screenshot goes here.
-->
