import { createRoot } from "react-dom/client";
import registry from "../i18n/locales.json";
import type source from "../i18n/messages/en.json";
import "./style.css";
import { FixKey } from "./fix-key";

const catalogs = import.meta.webpackContext("../i18n/messages", {
  regExp: /\.json$/,
  mode: "lazy",
});

async function main() {
  const lang =
    registry.locales.find(({ lang }) => lang === document.documentElement.lang)
      ?.lang ?? registry.sourceLang;
  const messages = (await catalogs(`./${lang}.json`)) as typeof source;
  const prefix = lang === registry.sourceLang ? "/" : `/${lang}/`;
  const isProduct = /\/products\/fix-key\/?$/.test(window.location.pathname);
  createRoot(document.getElementById("root")!).render(
    <>
      <a className="skip-link" href="#content">
        {messages.skipContent}
      </a>
      <header className="site-header">
        <a className="brand" href={prefix}>
          delino<span className="brand-dot">.</span>
        </a>
        {isProduct && (
          <nav aria-label="FixKey">
            <a href="#features">{messages.features}</a>
            <a href="#how-it-works">{messages.howItWorks}</a>
          </nav>
        )}
        <label>
          {messages.language}
          <select
            value={lang}
            onChange={(event) => {
              const selected = event.target.value;
              const path =
                selected === registry.sourceLang ? "/" : `/${selected}/`;
              window.location.assign(
                `${isProduct ? `/${selected}/products/fix-key/` : path}${window.location.search}${window.location.hash}`,
              );
            }}
          >
            {registry.locales.map(({ lang, label }) => (
              <option key={lang} value={lang} lang={lang}>
                {label}
              </option>
            ))}
          </select>
        </label>
      </header>
      <main id="content">
        {isProduct ? (
          <FixKey messages={messages} />
        ) : (
          <div className="home">
            <h1>Delino</h1>
            <a href={`/${lang}/products/fix-key/`}>FixKey →</a>
          </div>
        )}
      </main>
      <footer>
        <span>{messages.footerNote}</span>
        <div>
          <a href={`https://docs.delino.io${prefix}`}>
            {messages.documentation}
          </a>
          <a href="https://github.com/delinoio/apps">{messages.sourceCode}</a>
        </div>
      </footer>
    </>,
  );
}

main().catch(() => {
  console.error(
    JSON.stringify({
      event: "locale_load_failed",
      language: document.documentElement.lang,
    }),
  );
});
