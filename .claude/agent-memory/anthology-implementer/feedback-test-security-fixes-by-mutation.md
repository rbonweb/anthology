---
name: 'Prove a security fix is pinned by deleting it'
description: 'After a fix round, mutate each guard (delete the check) and confirm a test goes red; a passing run alone does not show that'
metadata:
  type: feedback
  modified: 2026-11-22
---

After each security fix, edit the fix out (git checkout the file afterwards, never sed -i) and run the targeted tests: a mutant that survives means the test only passes because another check refuses the same input. Use inputs that only the new check refuses (for example ./*.pem, not *.pem, which an older glob rule already refused).
Why: in the P3 review the cross-check with analyze could be replaced by a no-op and no test failed, because the program table refused every case the cross-check refused.
How to apply: one mutant per fix, in the container, restore with git checkout before the next one.
