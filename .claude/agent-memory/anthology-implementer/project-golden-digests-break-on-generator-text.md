---
name: 'Difftest golden digests break when generator text or lib output changes'
description: 'Big cases are stored as sha256 and resolved from the surface; any rename in generator data or in text the lib renders needs a repin, and goldens cannot be re-frozen without the Python oracle'
metadata:
  type: project
  modified: 2026-11-22
---

Cases over 8 KB are stored as case_sha256 and re-made from the surface seeds and generator. Renaming generator data, or library text the generator renders (for example the sync script name in upstream_watch), changes the digest and golden check says the surface no longer makes a case with that id and digest. golden bless only re-records result diffs, so it cannot fix it.
**Why:** the Python oracle is deleted, so freeze is impossible; the golden itself is the record.
**How to apply:** use a temporary local patch (not committed) that takes the surface current data for each case id (mapping old ids to renamed ids), keeps expect and marks of explained cases with the same rename applied, then run golden bless outside CI. Prove the result by diffing the old golden, renamed, against the new one: only digests, file hashes and sort order may differ.
