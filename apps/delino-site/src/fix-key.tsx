import type source from "../i18n/messages/en.json";

export function FixKey({ messages: m }: { messages: typeof source }) {
  const releases = "https://github.com/delinoio/apps/releases?q=fixkey%40";
  return (
    <>
      <section className="hero" aria-labelledby="hero-title">
        <p className="eyebrow">{m.eyebrow}</p>
        <h1 id="hero-title">
          {m.heroTitle}
          <br />
          <span>{m.heroAccent}</span>
        </h1>
        <p className="hero-body">{m.heroBody}</p>
        <div className="actions">
          <a className="button primary" href={releases}>
            {m.releases} <span aria-hidden="true">↗</span>
          </a>
          <a className="button secondary" href="#how-it-works">
            {m.seeHow} <span aria-hidden="true">↓</span>
          </a>
        </div>
        <p className="hero-note">{m.heroNote}</p>
        <div className="demo">
          <div className="demo-bar">
            <span className="window-dots" aria-hidden="true">
              <i />
              <i />
              <i />
            </span>
            <span>{m.demoLabel}</span>
            <span className="demo-brand">FixKey</span>
          </div>
          <div className="demo-content">
            <div>
              <span className="demo-label">{m.before}</span>
              <p className="typed" lang="en">
                dkssud
              </p>
            </div>
            <span className="conversion" aria-hidden="true">
              →
            </span>
            <div>
              <span className="demo-label">{m.after}</span>
              <p className="corrected" lang="ko">
                안녕
                <span className="caret" aria-hidden="true" />
              </p>
            </div>
          </div>
          <p className="demo-note">{m.demoNote}</p>
        </div>
      </section>
      <section
        id="features"
        className="section"
        aria-labelledby="features-title"
      >
        <p className="eyebrow">FixKey</p>
        <h2 id="features-title">{m.featureHeading}</h2>
        <p className="section-intro">{m.featureIntro}</p>
        <div className="feature-grid">
          {[
            ["한/A", m.featureOneTitle, m.featureOneBody],
            ["↳", m.featureTwoTitle, m.featureTwoBody],
            ["⌫", m.featureThreeTitle, m.featureThreeBody],
          ].map(([icon, title, body]) => (
            <article className="feature" key={title}>
              <span className="feature-icon" aria-hidden="true">
                {icon}
              </span>
              <h3>{title}</h3>
              <p>{body}</p>
            </article>
          ))}
        </div>
      </section>
      <section
        id="how-it-works"
        className="section how"
        aria-labelledby="how-title"
      >
        <h2 id="how-title">{m.howHeading}</h2>
        <ol className="steps">
          {[
            [m.stepOneTitle, m.stepOneBody],
            [m.stepTwoTitle, m.stepTwoBody],
            [m.stepThreeTitle, m.stepThreeBody],
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
        <h2 id="faq-title">{m.faqHeading}</h2>
        <div>
          {[
            [m.faqOneTitle, m.faqOneBody],
            [m.faqTwoTitle, m.faqTwoBody],
            [m.faqThreeTitle, m.faqThreeBody],
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
        <h2>{m.closingTitle}</h2>
        <p>{m.closingBody}</p>
        <a className="button primary" href={releases}>
          {m.releases} <span aria-hidden="true">↗</span>
        </a>
      </section>
    </>
  );
}
