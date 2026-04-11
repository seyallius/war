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

[group('Git')]
amend:
    git commit -a --amend

[group('Git')]
rebase n="3":
    git rebase -i HEAD~{{ n }}
