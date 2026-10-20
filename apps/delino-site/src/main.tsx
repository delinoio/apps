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
    <main>
      <header>
        <h1>Delino</h1>
        <label>
          {messages.language}
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
      <a href={`https://docs.delino.io${prefix}`}>{messages.documentation}</a>
    </main>,
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
