# Public inspiration gallery

The gallery uses a reviewed snapshot of public NovelAI quick-start image IDs and positive prompts. It does not read member generations, call an upstream generation API, or add load to the gateway queue.

The scheduled GitHub Actions workflow checks the official page weekly and opens a pull request only when image IDs or prompts change. The pull request is reviewable and does not deploy the site. To refresh locally:

```sh
python3 tools/sync-novelai-gallery.py
cd web
pnpm exec biome format --write src/pages/landing/gallery-data.json
```

The tool reads the public image page, discovers its same-origin JavaScript bundles, and extracts the image list and Description strings without executing upstream JavaScript. Missing prompts, unexpected list sizes, network failures or changed formats stop the update; the previous snapshot remains intact. It leaves the snapshot untouched when there are no changes. Review the generated JSON diff before merging.

Image clicks copy that example's positive prompt using the existing clipboard helper. A short overlay reports success or failure, resets after two seconds, and cleans up its timer on unmount. Images load directly from the official static host without a referrer, with a source link if loading fails. The public CSP permits only that additional image host; connect-src remains self-only.

The original site's click handler imports prompt/settings metadata into its editor and shows Copied! for two seconds. This site has no generation editor, so it writes the prompt to the clipboard instead. Negative prompts and generation settings are not appended to the positive prompt.
