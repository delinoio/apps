import { Button } from "@delino/ui/components/button";
import { Card } from "@delino/ui/components/card";
import type source from "../i18n/messages/en.json";

export function FixKey({ messages: m }: { messages: typeof source }) {
  const releases = "https://github.com/delinoio/apps/releases?q=fixkey%40";
  return (
    <>
      <section className="hero" aria-labelledby="hero-title">
        <p className="eyebrow">{m["fixkey.eyebrow"]}</p>
        <h1 id="hero-title">
          {m["fixkey.heroTitle"]}
          <br />
          <span>{m["fixkey.heroAccent"]}</span>
        </h1>
        <p className="hero-body">{m["fixkey.heroBody"]}</p>
        <div className="actions">
          <Button asChild>
            <a className="button primary" href={releases}>
              {m["fixkey.releases"]} <span aria-hidden="true">↗</span>
            </a>
          </Button>
          <Button asChild variant="secondary">
            <a className="button secondary" href="#how-it-works">
              {m["fixkey.seeHow"]} <span aria-hidden="true">↓</span>
            </a>
          </Button>
        </div>
        <p className="hero-note">{m["fixkey.heroNote"]}</p>
        <Card className="demo">
          <div className="demo-bar">
            <span className="window-dots" aria-hidden="true">
              <i />
              <i />
              <i />
            </span>
            <span>{m["fixkey.demoLabel"]}</span>
            <span className="demo-brand">FixKey</span>
          </div>
          <div className="demo-content">
            <div>
              <span className="demo-label">{m["fixkey.before"]}</span>
              <p className="typed" lang="en">
                dkssud
              </p>
            </div>
            <span className="conversion" aria-hidden="true">
              →
            </span>
            <div>
              <span className="demo-label">{m["fixkey.after"]}</span>
              <p className="corrected" lang="ko">
                안녕
                <span className="caret" aria-hidden="true" />
              </p>
            </div>
          </div>
          <p className="demo-note">{m["fixkey.demoNote"]}</p>
        </Card>
      </section>
      <section
        id="features"
        className="section"
        aria-labelledby="features-title"
      >
        <p className="eyebrow">FixKey</p>
        <h2 id="features-title">{m["fixkey.featureHeading"]}</h2>
        <p className="section-intro">{m["fixkey.featureIntro"]}</p>
        <div className="feature-grid">
          {[
            ["한/A", m["fixkey.featureOneTitle"], m["fixkey.featureOneBody"]],
            ["↳", m["fixkey.featureTwoTitle"], m["fixkey.featureTwoBody"]],
            ["⌫", m["fixkey.featureThreeTitle"], m["fixkey.featureThreeBody"]],
          ].map(([icon, title, body]) => (
            <Card asChild key={title}>
              <article className="feature">
                <span className="feature-icon" aria-hidden="true">
                  {icon}
                </span>
                <h3>{title}</h3>
                <p>{body}</p>
              </article>
            </Card>
          ))}
        </div>
      </section>
      <section
        id="how-it-works"
        className="section how"
        aria-labelledby="how-title"
      >
        <h2 id="how-title">{m["fixkey.howHeading"]}</h2>
        <ol className="steps">
          {[
            [m["fixkey.stepOneTitle"], m["fixkey.stepOneBody"]],
            [m["fixkey.stepTwoTitle"], m["fixkey.stepTwoBody"]],
            [m["fixkey.stepThreeTitle"], m["fixkey.stepThreeBody"]],
          ].map(([title, body], index) => (
            <li key={title}>
              <span className="step-number" aria-hidden="true">
                0{index + 1}
              </span>
              <div>
                <h3>{title}</h3>
                <p>{body}</p>
              </div>
            </li>
          ))}
        </ol>
      </section>
      <section className="section faq" aria-labelledby="faq-title">
        <h2 id="faq-title">{m["fixkey.faqHeading"]}</h2>
        <div>
          {[
            [m["fixkey.faqOneTitle"], m["fixkey.faqOneBody"]],
            [m["fixkey.faqTwoTitle"], m["fixkey.faqTwoBody"]],
            [m["fixkey.faqThreeTitle"], m["fixkey.faqThreeBody"]],
          ].map(([title, body]) => (
            <details key={title}>
              <summary>{title}</summary>
              <p>{body}</p>
            </details>
          ))}
        </div>
      </section>
      <section className="closing">
        <span className="key-logo" aria-hidden="true">
          F<span>↵</span>
        </span>
        <h2>{m["fixkey.closingTitle"]}</h2>
        <p>{m["fixkey.closingBody"]}</p>
        <Button asChild>
          <a className="button primary" href={releases}>
            {m["fixkey.releases"]} <span aria-hidden="true">↗</span>
          </a>
        </Button>
      </section>
    </>
  );
}
