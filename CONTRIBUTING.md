# Contributing

Stranger Thoughts is a small static site. A new public note is a JSON file plus a rebuild. The build writes the HTML that GitHub Pages serves.

## Add a nightly thought

1. Copy the template.

   ```bash
   cp data/posts/_template.json data/posts/the-note-slug.json
   ```

   The filename and the `slug` field must match. Use lowercase words separated by hyphens. Files whose names start with `_` are not published.

2. Replace every placeholder.
   - `title` — the note's title, in American English.
   - `date` — `YYYY-MM-DD`, today or earlier. If the date is after `checked` in `data/site.json`, update `checked` to the same day or later. The build stops when a note is dated after `checked`.
   - `kicker` — a short label such as `Craft note`, `On replies`, or `A prompt`.
   - `description` — one or two sentences. This is the index line and the meta description.
   - `blocks` — at least two paragraphs (`"type": "p"`). Add `"type": "h2"` where the note turns. Keep paragraphs concrete. A note that only announces itself will fail the length check.

3. Keep the desk anonymous, and keep other people anonymous too.
   - No legal name, face, portrait, location, rank, unit, or service history.
   - No social handle, and no link to X, Instagram, or any other profile. The brand account is not open yet.
   - No client's page, and no private story presented as if someone sent it.
   - No prices, payment, donations, or a street address. The mailing line stays `Mailing address coming soon`.

4. Rebuild from the repository root.

   ```bash
   python3 scripts/build.py
   ```

   That refreshes the new page at `thoughts/<slug>/`, the thoughts index, the three recent notes on the home page, older/newer links on the neighboring notes, `sitemap.xml`, and `robots.txt`.

5. Commit the JSON file and every generated file the rebuild changed. Do not hand-edit the HTML. The next build overwrites it.

## What the build will refuse

- A slug that does not match the filename, or the template slug `your-slug-here`.
- A date that is not real, or a date after `data/site.json`'s `checked` value.
- A `CNAME` file while the custom domain is still `not_live`.
- A mailing address other than `Mailing address coming soon`.
- A waitlist address at a personal inbox (Gmail and the like).
- A social URL or an `@handle` in the copy.
- A street-style address or a post-office box in the copy.

## Preview

Links are written for the GitHub Pages project URL. Serve the repo under that prefix:

```bash
mkdir -p /tmp/st-preview
ln -sfn "$PWD" /tmp/st-preview/stranger-thoughts
python3 -m http.server 8080 --directory /tmp/st-preview
```

Open `http://localhost:8080/stranger-thoughts/thoughts/<slug>/`.
