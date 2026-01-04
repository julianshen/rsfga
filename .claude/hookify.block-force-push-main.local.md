---
name: block-force-push-main
enabled: true
event: bash
action: block
pattern: git\s+push\s+.*--force.*\s+(origin\s+)?(main|master)
---

🚫 **Force Push to Main/Master BLOCKED**

**CLAUDE.md Git Safety Rule**: "NEVER run force push to main/master, warn the user if they request it"

**Why this is dangerous**:
- Force push rewrites history on the main branch
- Can cause data loss for other contributors
- Breaks git history for anyone who has pulled the branch
- Violates git collaboration best practices

**What you attempted**:
```bash
git push --force origin main
```

**Safe alternatives**:

1. **For fixing recent commits on main**:
   ```bash
   git revert HEAD
   git push origin main
   ```

2. **For syncing with remote**:
   ```bash
   git pull --rebase origin main
   git push origin main
   ```

3. **For feature branches** (force push is OK):
   ```bash
   git push --force origin feature/your-branch
   ```

**If you REALLY need to force push main**:
- Get team consensus first
- Notify all contributors
- Have the user run the command manually
- Document the reason in team chat

**This rule protects**:
- Team collaboration
- Git history integrity
- Code review process
- Deployment stability
