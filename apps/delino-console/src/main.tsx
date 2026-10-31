import { createRoot } from "react-dom/client";
import registry from "../i18n/locales.json";
import type source from "../i18n/messages/en.json";
import "./style.css";

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
  createRoot(document.getElementById("root")!).render(
    <>
      <a className="skip-link" href="#content">
        {messages["common.skipContent"]}
      </a>
      <header className="site-header">
        <a className="brand" href={prefix}>
          delino console
        </a>
        <label>
          {messages["common.language"]}
          <select
            value={lang}
            onChange={(event) => {
              const selected = event.target.value;
              const path =
                selected === registry.sourceLang ? "/" : `/${selected}/`;
              window.location.assign(
                `${path}${window.location.search}${window.location.hash}`,
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
      <main id="content" tabIndex={-1}>
        <h1>{messages["console.title"]}</h1>
        <p>{messages["console.preparing"]}</p>
      </main>
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
  // Recovery copy is embedded in each HTML entry so a failed catalog chunk
  // cannot prevent localized recovery. Retain this while catalogs load lazily.
  document.getElementById("root")!.hidden = true;
  document.getElementById("locale-error")!.hidden = false;
  document
    .getElementById("reload")!
    .addEventListener("click", () => window.location.reload());
  document.getElementById("error-title")!.focus();
});
