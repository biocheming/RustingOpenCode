---
name: folder-inspector
description: Structured folder purpose analysis with concise evidence.
---

# Folder Inspector Skill

When user asks what a folder does, follow this workflow:

1. Run `ls` for top-level files.
2. Pick 2-5 representative files and run `read`.
3. Summarize:
- Main purpose
- Key files and roles
- Next actionable steps

Output constraints:

- Keep summary concise.
- Avoid dumping full file contents.
- Prefer bullet points with file references.
