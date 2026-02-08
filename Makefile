SHELL := /bin/bash

.PHONY: test skills-sync

# Keep "test" as the single entrypoint for local enforcement checks.
test: skills-sync

# Verifies that the same skill installed for Claude and Codex is byte-identical.
skills-sync:
	./scripts/check_skills_sync.sh

