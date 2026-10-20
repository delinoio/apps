import { createRoot } from "react-dom/client";
import "./style.css";

enum Language {
  English = "en",
  Korean = "ko",
}

const language =
  new URLSearchParams(window.location.search).get("lang") === Language.Korean
    ? Language.Korean
    : Language.English;
const messages = {
  [Language.English]: { documentation: "Documentation", language: "Language" },
  [Language.Korean]: { documentation: "문서", language: "언어" },
} satisfies Record<Language, { documentation: string; language: string }>;
const text = messages[language];

document.documentElement.lang = language;

createRoot(document.getElementById("root")!).render(
  <header>
    <h1>Delino</h1>
    <nav aria-label={text.documentation}>
      <a
        href={`https://docs.delino.io/${language === Language.Korean ? "ko/" : ""}`}
      >
        {text.documentation}
      </a>
      <label>
        {text.language}
        <select
          value={language}
          onChange={(event) => {
            const url = new URL(window.location.href);
            url.searchParams.set("lang", event.target.value);
            window.location.assign(url);
          }}
        >
          <option value={Language.English} lang="en">
            English
          </option>
          <option value={Language.Korean} lang="ko">
            한국어
          </option>
        </select>
      </label>
    </nav>
  </header>,
);
