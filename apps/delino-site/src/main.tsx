import { Label } from "@packages/ui/components/label";
import { NativeSelect } from "@packages/ui/components/native-select";
import { createRoot } from "react-dom/client";
import registry from "../i18n/locales.json";
import type source from "../i18n/messages/en.json";
import "./style.css";
import { Home } from "./home";

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
      <header className="site-header company-header">
        <a className="brand" href={prefix}>
          delino<span className="brand-dot">.</span>
        </a>
        <nav className="company-nav" aria-label={messages["delino.navigation"]}>
          <a href="#about">{messages["delino.navAbout"]}</a>
          <a href="#products">{messages["delino.navProducts"]}</a>
          <a href="#approach">{messages["delino.navApproach"]}</a>
        </nav>
        <Label>
          {messages["common.language"]}
          <NativeSelect
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
          </NativeSelect>
        </Label>
      </header>
      <main id="content">
        <Home messages={messages} prefix={prefix} />
      </main>
      <footer className="company-footer">
        <div className="company-footer-brand">
          <a className="brand" href={prefix}>
            delino<span className="brand-dot">.</span>
          </a>
          <span>{messages["delino.footerTagline"]}</span>
        </div>
        <div>
          <a href={`https://docs.delino.io${prefix}`}>
            {messages["common.documentation"]}
          </a>
          <a href="https://github.com/delinoio/apps">
            {messages["common.sourceCode"]}
          </a>
        </div>
        <small>{messages["common.footerNote"]}</small>
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
  const root = document.getElementById("root");
  if (root) {
    // Keep recovery independent of the catalog chunk that failed to load.
    root.replaceChildren();
    const panel = document.createElement("main");
    panel.className = "locale-error";
    panel.lang = "en";
    const title = document.createElement("h1");
    title.textContent = "This page could not load.";
    const message = document.createElement("p");
    message.textContent = "Please reload to try again.";
    const reload = document.createElement("button");
    reload.className = "button primary";
    reload.textContent = "Reload";
    reload.addEventListener("click", () => window.location.reload());
    panel.append(title, message, reload);
    root.append(panel);
  }
});
