#!/usr/bin/env just --justfile

default:
    @just --list

# ----------------------------------------------------------------
# Development
# ----------------------------------------------------------------

[group('Development')]
treeclip dir="":
    treeclip run {{ dir }} -f -t -c -v --stats

# ----------------------------------------------------------------
# Code Quality
# ----------------------------------------------------------------

[group('Code Quality')]
clippy:
    cargo clippy -- -D warnings

# ----------------------------------------------------------------
# Dependency
# ----------------------------------------------------------------

[group('Dependency')]
vendor:
    cargo vendor vendor --versioned-dirs --no-delete

[group('Dependency')]
clean-vendor:
    rm -rf ./vendor

# ----------------------------------------------------------------
# Git & Version Control
# ----------------------------------------------------------------

# Commit staged changes with amend.
[group('Git')]
amend:
    git commit -a --amend

# Rebase current branch to the specified number of commits (Usage: just rebase 5)
[group('Git')]
rebase n="3":
    git rebase -i HEAD~{{ n }}

# Copy the git diff output to clipboard.
[group('Git')]
[linux]
diff-cp:
    git diff | xclip -selection clipboard

# Copy the git diff output to clipboard.
[group('Git')]
[windows]
diff-cp:
    git diff | /c/Windows/System32/clip.exe

# Print the commits done today.
[group('Git')]
today:
    git log --since="today 00:00:00" --until="today 23:59:59" --oneline
