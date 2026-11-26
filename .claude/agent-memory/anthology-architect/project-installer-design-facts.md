---
name: 'One-command installer: facts that shaped the design'
description: 'npm name anthology is taken; tap repo absent; Coucou brand assets are not MIT; claude plugin install -y auto-accepts marketplace commands; setup preview/apply exists'
metadata:
  type: project
  modified: 2026-11-26
---

Facts checked on 2026-11-26 for the brew/npm installer design:
- npm: the name anthology belongs to an unrelated 2013 package; anthology-installer and the @rbonweb scope were free. Why: the package name is an owner call.
- rbonweb/homebrew-anthology does not exist yet; a tap named rbonweb/anthology needs that repo.
- Coucou (Louis-CFM/coucou): code is MIT but LICENSE-ASSETS.md reserves the name, the Mochi character, icons and sounds. A fork must rebrand. Why: it rules out shipping a fork as Coucou.
- Coucou mac cask lives in the main repo under Casks/coucou.rb, auto_updates true, needs macOS Sequoia; mac tags are vX.Y.Z with Coucou.zip; Linux tags linux-vX.Y.Z ship x86_64 AppImage, amd64 deb, rpm and SHA256SUMS; no Linux arm64 build.
- claude plugin install and update take -y and --accept-command, which accept a marketplace-declared command; an installer must never pass them blindly. --json gives one result line.
- claude plugin list --json gives installPath, version and errorDetails (engine-range-unsatisfied when Claude Code is older than engines.claude-code).
- The anthology binary has setup preview, apply (needs --expect-sha256 from the preview, has --skip-approve) and remove (has --preview). Why: an installer can show the settings diff and apply it only after a yes.
- The documented binary installer command is the guardrails hooks/bootstrap.sh with guard --plugin guardrails --quiet --budget 300; it prints status=present, installed or failed.
