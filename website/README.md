# Pablo website and docs

The public site is a static VitePress build whose source pages live in
[`../docs/guide`](../docs/guide). This keeps the rendered documentation and the
Markdown browsable on GitHub in sync.

```sh
cd website
npm ci
npm run dev
npm test
```

## Cloudflare Pages

For Git integration, configure the Pages project with:

- Root directory: `website`
- Build command: `npm run build`
- Build output directory: `dist`
- Node version: `24`

For a direct upload to the `pablo-runtime` Pages project after authenticating
Wrangler, run `npm run deploy`. The
site is static and requires no runtime variables, Pages Functions, or secrets.
The production URL is <https://runpablo.pages.dev>.

`.github/workflows/docs-pages.yml` builds pull requests and deploys changes from
`main`. Add `CLOUDFLARE_ACCOUNT_ID` and a `CLOUDFLARE_API_TOKEN` with Account →
Cloudflare Pages → Edit permission as GitHub Actions repository secrets. The
workflow does not expose either value to the static build.
