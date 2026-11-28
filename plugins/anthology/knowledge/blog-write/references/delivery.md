# Blog Write Delivery Checklist

See Phase 6.5 of SKILL.md.

## URL And Asset Safety

- Allow `http` and `https` only.
- Reject `javascript:`, `data:`, and `file:` URLs.
- Resolve DNS and reject loopback, private, link-local, multicast, and reserved IP ranges.
- Disable redirects or validate the final URL with the same rules.
- Use tight timeouts and response-size caps.
- Prefer official provider APIs and Openverse so license and creator metadata are preserved.
- Download assets locally and store attribution with the draft. Never hotlink an image in the delivered article.

## Agent Report

Hand back in your agent report:

```
## Blog Post Complete: [Title]

### Template Used
- [Template name] or "generic outline"

### Statistics
- [N] sourced statistics from tier 1-3 sources
- [N] unique sources cited

### Visual Elements
- Cover image: [local path, source, model ID or license]
- [N] inline images
- [N] SVG charts
- [N] YouTube video embeds

### Dual-Optimization Elements
- Summary box: present
- Information gain markers: [N]
- Evidence-backed sections: [N]
- Internal links: [N] resolved

### Structure
- [N] H2 sections with answer-first formatting
- [N] FAQ items when warranted
- Word count: ~[N] words

### Fallbacks and skips
- [One line per sub-skill fallback or skipped step: what changed and why, or "none"]

### Optional Editorial Style Diagnostics
- Sentence-length variation: [descriptive observation]
- Configured style-list terms: [N found]
- Voice fit: [brief observation]
- These diagnostics do not infer authorship or affect Google/readiness scoring.

### Next Steps
- Run `/blog analyze <file>` to verify quality score
- Generate VideoObject schema with `/blog schema <file>` when videos are present
```

## Optional Platform Embedding Examples

Load these examples only when the draft uses images or charts.

#### 5i. Image Embedding

Images are local files in the draft folder with lowercase slug-style names.

Standard markdown:
```markdown
![Descriptive alt text, topic keywords naturally](diagram-core-web-vitals-thresholds.webp)
```

MDX (a plain markdown image works; use your framework's image component only when
the project already does):
```mdx
![Descriptive alt text, topic keywords naturally](diagram-core-web-vitals-thresholds.webp)
```

- Place images after H2 headings, before body text
- Space evenly throughout the post (not clustered)
- Alt text should be a full descriptive sentence

#### 5j. Chart Embedding

Standard markdown/HTML:
```html
<figure>
  <svg viewBox="0 0 560 380" ...>...</svg>
  <figcaption>Source: [Source Name], [Year]</figcaption>
</figure>
```

MDX format:
```mdx
<figure className="chart-container" style={{margin: '2.5rem 0', textAlign: 'center', padding: '1.5rem', borderRadius: '12px'}}>
  <svg viewBox="0 0 560 380" ...>...</svg>
</figure>
```

Give each chart a unique id so aria-labelledby ids never collide.
