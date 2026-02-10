# AWS Server Deploy Fix Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make `make deploy` resilient to EC2 public IP changes by auto-discovering the current server via AWS, while still allowing manual overrides.

**Architecture:** Add a small script that resolves the SSH target (`ubuntu@<ip>`) from (1) explicit `SERVER`, (2) `TRADING_SERVER_IP`, (3) AWS EC2 instance tag/name lookup, otherwise a safe placeholder. Wire it into the Makefile as the default `SERVER` value.

**Tech Stack:** GNU Make, bash, AWS CLI v2, SSH/SCP.

---

### Task 1: Confirm Current EC2 Instance Target

**Files:**
- None

**Step 1: Query AWS for running instances**

Run:
```bash
aws ec2 describe-instances \
  --filters Name=instance-state-name,Values=running \
  --query 'Reservations[].Instances[].{id:InstanceId,name:Tags[?Key==`Name`].Value|[0],public_ip:PublicIpAddress}' \
  --output table
```

Expected: A row for the current server (e.g. `name=trading-bot` with a non-empty `public_ip`).

---

### Task 2: Add `scripts/aws_find_server.sh`

**Files:**
- Create: `scripts/aws_find_server.sh`

**Step 1: Implement the resolver script**

Rules:
- If `$SERVER` is already set by Make/user, do nothing (Make handles this; script only prints a default).
- If `$TRADING_SERVER_IP` is set, print `ubuntu@$TRADING_SERVER_IP`.
- Else attempt AWS lookup in region from `aws configure get region`, falling back to `eu-west-2`.
- Prefer instance with tag `Name=trading-bot` in `running` state.
- If lookup fails, print `ubuntu@YOUR_SERVER_IP` (non-empty fallback).

**Step 2: Quick local verification**

Run:
```bash
./scripts/aws_find_server.sh
```

Expected: Prints a single `ubuntu@<ip>` value (not empty).

---

### Task 3: Wire Resolver Into `Makefile`

**Files:**
- Modify: `Makefile`

**Step 1: Use resolver as default `SERVER`**

Change `SERVER ?= ubuntu@YOUR_SERVER_IP` to something like:
```make
SERVER ?= $(shell ./scripts/aws_find_server.sh)
```

Keep overrides working:
- `make deploy SERVER=ubuntu@1.2.3.4` should still work.
- `make deploy TRADING_SERVER_IP=1.2.3.4` should work.

**Step 2: Verification without deploying**

Run:
```bash
make -n deploy | head -20
```

Expected: `ssh`/`scp` commands show a non-placeholder `$(SERVER)` value when AWS lookup succeeds.

---

### Task 4: Remove Hard-Coded Old IP In Helper Script Comment

**Files:**
- Modify: `deploy/setup-cloudflared.sh`

**Step 1: Update comment**

Change the comment “Run this on the target server (ubuntu@…).” to a generic “Run this on the target server.”.

**Step 2: Verification**

Run:
```bash
rg -n "3\\.8\\.206\\.244" -S .
```

Expected: No matches.

---

### Task 5: Full Verification

**Files:**
- None

**Step 1: Run standard test suite**

Run:
```bash
make test
```

Expected: PASS.

**Step 2: Confirm SSH port is reachable (non-deploying)**

Run:
```bash
ssh -i ~/git_projects/trading/trading-bot.pem \
  -o BatchMode=yes -o ConnectTimeout=7 -o StrictHostKeyChecking=accept-new \
  "$(./scripts/aws_find_server.sh)" 'echo ok'
```

Expected: Prints `ok`.

---

### Task 6: Commit And Open PR

**Files:**
- None

**Step 1: Commit**

Run:
```bash
git add Makefile scripts/aws_find_server.sh deploy/setup-cloudflared.sh docs/plans/2026-02-10-aws-server-deploy-fix.md
git commit -m "chore: auto-discover deploy server via AWS"
```

**Step 2: Push and open PR**

Run:
```bash
git push -u origin codex/aws-server-discovery
```

Then create the PR (via `gh pr create` if available, or GitHub UI).
