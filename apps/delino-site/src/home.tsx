import { Button } from "@packages/ui/components/button";
import { Card } from "@packages/ui/components/card";
import type source from "../i18n/messages/en.json";

function CommitIllustration() {
  return (
    <svg
      className="commit-illustration"
      viewBox="0 0 440 340"
      aria-hidden="true"
    >
      <path d="M70 55v180" fill="none" stroke="#202b3c" strokeWidth="2" />
      {[55, 145, 235].map((y, index) => (
        <g key={y}>
          <circle
            cx="70"
            cy={y}
            r="11"
            fill={index === 0 ? "#246ee0" : "#fff"}
            stroke={index === 0 ? "#246ee0" : "#202b3c"}
            strokeWidth="2"
          />
          <rect
            x="108"
            y={y - 30}
            width="272"
            height="62"
            rx="10"
            fill="#fff"
          />
          <rect
            x="126"
            y={y - 11}
            width={100 - index * 12}
            height="9"
            rx="4"
            fill="#c6d2e3"
          />
          <rect
            x="126"
            y={y + 7}
            width={150 + index * 12}
            height="8"
            rx="4"
            fill="#e7edf5"
          />
          <rect
            x="324"
            y={y - 11}
            width="38"
            height="9"
            rx="4"
            fill="#e7edf5"
          />
        </g>
      ))}
      <path
        d="M42 310C88 278 105 323 150 296S207 285 241 303S302 302 328 288S361 292 389 275"
        fill="none"
        stroke="#246ee0"
        strokeWidth="3"
      />
      <circle
        cx="389"
        cy="275"
        r="5"
        fill="#fff"
        stroke="#246ee0"
        strokeWidth="3"
      />
    </svg>
  );
}

export function Home({
  messages: m,
  prefix,
}: {
  messages: typeof source;
  prefix: string;
}) {
  const docs = `https://docs.delino.io${prefix}`;
  return (
    <div className="company-home">
      <section
        className="company-hero company-container"
        aria-labelledby="company-title"
      >
        <div>
          <p className="company-eyebrow">{m["delino.eyebrow"]}</p>
          <h1 id="company-title">{m["delino.heroTitle"]}</h1>
          <p className="company-lead">{m["delino.heroBody"]}</p>
          <div className="company-actions">
            <Button asChild>
              <a className="button primary" href="#products">
                {m["delino.exploreProducts"]} <span aria-hidden="true">→</span>
              </a>
            </Button>
            <a className="company-text-link" href="#about">
              {m["delino.about"]} <span aria-hidden="true">→</span>
            </a>
          </div>
        </div>
        <figure className="company-hero-art">
          <div className="company-art">
            <CommitIllustration />
          </div>
          <figcaption>{m["delino.illustrationCaption"]}</figcaption>
        </figure>
      </section>
      <section
        id="about"
        className="company-about company-container company-section"
        aria-labelledby="about-title"
      >
        <p className="company-eyebrow">01 / {m["delino.aboutLabel"]}</p>
        <div className="company-about-grid">
          <h2 id="about-title">{m["delino.aboutTitle"]}</h2>
          <p>{m["delino.aboutBody"]}</p>
        </div>
      </section>
      <section
        id="products"
        className="company-products company-container company-section"
        aria-labelledby="products-title"
      >
        <p className="company-eyebrow">02 / {m["delino.productsLabel"]}</p>
        <h2 id="products-title">{m["delino.productsTitle"]}</h2>
        <div className="company-product-grid">
          <Card asChild>
            <article className="company-product">
              <figure className="company-product-art">
                <CommitIllustration />
                <figcaption>{m["delino.illustrativeView"]}</figcaption>
              </figure>
              <div className="company-product-copy">
                <p className="company-category">{m["delino.developerTools"]}</p>
                <h3>Git Tracker</h3>
                <p>{m["delino.gitTrackerBody"]}</p>
                <div className="company-product-links">
                  <a href={`${docs}services/git-tracker/`}>
                    {m["delino.readDocumentation"]}{" "}
                    <span aria-hidden="true">↗</span>
                  </a>
                  <a href="https://github.com/delinoio/apps">
                    {m["delino.viewSource"]} <span aria-hidden="true">↗</span>
                  </a>
                </div>
              </div>
            </article>
          </Card>
        </div>
        <p className="company-availability">{m["delino.availability"]}</p>
      </section>
      <section
        id="approach"
        className="company-approach"
        aria-labelledby="approach-title"
      >
        <div className="company-container company-section">
          <p className="company-eyebrow">03 / {m["delino.approachLabel"]}</p>
          <h2 id="approach-title">{m["delino.approachTitle"]}</h2>
          <ol className="company-principles">
            {[
              [m["delino.principleOneTitle"], m["delino.principleOneBody"]],
              [m["delino.principleTwoTitle"], m["delino.principleTwoBody"]],
              [m["delino.principleThreeTitle"], m["delino.principleThreeBody"]],
            ].map(([title, body], index) => (
              <li key={title}>
                <span className="company-number" aria-hidden="true">
                  0{index + 1}
                </span>
                <h3>{title}</h3>
                <p>{body}</p>
              </li>
            ))}
          </ol>
        </div>
      </section>
      <section
        className="company-contact company-container company-section"
        aria-labelledby="contact-title"
      >
        <h2 id="contact-title">{m["delino.contactTitle"]}</h2>
        <p>{m["delino.contactBody"]}</p>
        <div className="company-actions">
          <Button asChild>
            <a className="button primary" href="mailto:support@delino.io">
              {m["delino.contactAction"]} <span aria-hidden="true">↗</span>
            </a>
          </Button>
          <a className="company-text-link" href="mailto:support@delino.io">
            support@delino.io
          </a>
        </div>
      </section>
    </div>
  );
}
