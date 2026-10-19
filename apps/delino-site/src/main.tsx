// SPDX-License-Identifier: Apache-2.0
import { useState, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import "./styles.css";

const releaseUrl =
  "https://github.com/delinoio/apps/releases/tag/fixkey%40v0.1.7";
const downloadBase =
  "https://github.com/delinoio/apps/releases/download/fixkey%40v0.1.7/";

enum Platform {
  Mac = "macos-arm64",
  Windows = "windows-x64",
  Linux = "linux-x11-x64",
}

const platforms = [
  {
    id: Platform.Mac,
    name: "macOS",
    detail: "Apple Silicon · DMG",
    file: "FixKey-macos-arm64.dmg",
    symbol: "⌘",
  },
  {
    id: Platform.Windows,
    name: "Windows",
    detail: "64-bit · EXE",
    file: "FixKey-windows-x64.exe",
    symbol: "⊞",
  },
  {
    id: Platform.Linux,
    name: "Linux",
    detail: "X11 · 64-bit · AppImage",
    file: "FixKey-linux-x11-x64.AppImage",
    symbol: "⌁",
  },
];

function Icon({
  name,
  className = "",
}: {
  name:
    "arrow" | "download" | "check" | "shield" | "sparkles" | "undo" | "github";
  className?: string;
}) {
  const paths: Record<typeof name, ReactNode> = {
    arrow: <path d="M4 12h15m-6-6 6 6-6 6" />,
    download: (
      <>
        <path d="M12 3v12m-5-5 5 5 5-5M4 16v5h16v-5" />
      </>
    ),
    check: <path d="m5 12 4 4L19 6" />,
    shield: (
      <>
        <path d="m12 3 8 3v6c0 5-8 9-8 9s-8-4-8-9V6l8-3Z" />
        <path d="m8 12 3 3 5-6" />
      </>
    ),
    sparkles: (
      <>
        <path d="m12 3 2.5 6.5L21 12l-6.5 2.5L12 21l-2.5-6.5L3 12l6.5-2.5L12 3Z" />
        <path d="m20 2 .5 1.5L22 4l-1.5.5L20 6l-.5-1.5L18 4l1.5-.5L20 2Z" />
      </>
    ),
    undo: (
      <>
        <path d="m9 4-6 6 6 6M3 10h11a6 6 0 0 1 0 12" />
      </>
    ),
    github: (
      <>
        <path d="M9 20c-4 1-4-2-6-2m12 4v-3.4a3 3 0 0 0-.8-2.3c2.7-.3 5.5-1.3 5.5-6A4.7 4.7 0 0 0 18.5 7a4.3 4.3 0 0 0-.1-3.3S17.4 3.4 15 5a11.4 11.4 0 0 0-6 0C6.6 3.4 5.6 3.7 5.6 3.7A4.3 4.3 0 0 0 5.5 7a4.7 4.7 0 0 0-1.2 3.3c0 4.7 2.8 5.7 5.5 6a3 3 0 0 0-.8 2.3V22" />
      </>
    ),
  };
  return (
    <svg
      className={`icon ${className}`}
      width="20"
      height="20"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {paths[name]}
    </svg>
  );
}

function ProductMark({ small = false }: { small?: boolean }) {
  return (
    <span className={`product-mark ${small ? "small" : ""}`} aria-hidden="true">
      <svg viewBox="0 0 32 32" fill="none">
        <path
          d="m8 17 5 5L25 9"
          stroke="currentColor"
          strokeWidth="4"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
        <path
          d="M8 8h7M8 8v5"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          opacity=".55"
        />
      </svg>
    </span>
  );
}

const examples = [
  { before: "dkssud", after: "안녕", label: "안녕" },
  { before: "rkatk", after: "감사", label: "감사" },
  { before: "ㅗ디ㅣㅐ", after: "hello", label: "hello" },
];

function Demo() {
  const [example, setExample] = useState(0);
  const [corrected, setCorrected] = useState(false);
  const current = examples[example];
  return (
    <div className="demo-scene" id="demo">
      <div className="scene-grain" />
      <span className="scene-caption">
        <span /> LESS SWITCHING, MORE FLOW.
      </span>
      <div className="editor-window">
        <div className="window-bar">
          <div className="window-dots">
            <i />
            <i />
            <i />
          </div>
          <span>나의 작은 메모</span>
          <span className="editor-menu">···</span>
        </div>
        <div className="editor-content">
          <span className="note-date">A LITTLE NOTE TO MYSELF</span>
          <h3>오늘도, 생각을 이어서.</h3>
          <p>
            쓰고 싶은 말이 떠올랐을 때,
            <br />
            한/영 키는 잠깐 잊어도 괜찮아요.
          </p>
          <div className="demo-text" aria-live="polite">
            <span className={corrected ? "corrected-word" : "original-word"}>
              {corrected ? current.after : current.before}
            </span>
            <span className="caret" />
          </div>
          <div
            className={`correction-toast ${corrected ? "is-corrected" : ""}`}
          >
            <ProductMark small />
            <span>
              {corrected
                ? "의도한 단어로 바꿨어요"
                : "앗, 다른 배열로 입력했네요"}
            </span>
            {corrected && <Icon name="check" />}
          </div>
        </div>
        <div className="editor-bottom">
          <span>
            <span className="status-dot" />
            {corrected ? "교정 완료" : "교정 예시"}
          </span>
          <button
            onClick={() => setCorrected(!corrected)}
            className="demo-button"
          >
            {corrected ? "다시 보기" : "교정해 보기"}
            <Icon name={corrected ? "undo" : "sparkles"} />
          </button>
        </div>
      </div>
      <div className="floating-key key-english" aria-hidden="true">
        <span>A</span>
        <small>ENGLISH</small>
      </div>
      <div className="key-connector" aria-hidden="true">
        ↔
      </div>
      <div className="floating-key key-korean" aria-hidden="true">
        <span>한</span>
        <small>한국어</small>
        <span className="key-sparkle">✦</span>
      </div>
      <div className="flow-label">
        <Icon name="sparkles" /> 손끝의 실수까지, 자연스럽게.
      </div>
      <div className="demo-controls">
        <span>다른 예시도 눌러보세요</span>
        <div>
          {examples.map((item, index) => (
            <button
              key={item.before}
              aria-pressed={example === index}
              onClick={() => {
                setExample(index);
                setCorrected(false);
              }}
            >
              {item.before}
              <span>→</span>
              {item.label}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}

const faqs = [
  {
    question: "어떤 키보드 배열을 사용할 수 있나요?",
    answer:
      "한국어 두벌식과 US English QWERTY를 양방향으로 지원해요. 완성된 단어를 각 배열로 재구성한 뒤, 활성화된 언어 중 한 언어의 사전에서만 유효한 경우 교정해요. 여러 해석이 가능한 단어는 자동으로 바꾸지 않아요.",
  },
  {
    question: "입력한 내용이 외부로 전송되나요?",
    answer:
      "입력 내용, 문서 내용, 교정 기록은 저장하거나 전송하지 않아요. 교정은 기기 안에서 처리하며, 네트워크는 앱 업데이트 확인과 다운로드에만 사용해요. 텔레메트리나 원격 진단 로그 업로드도 없어요.",
  },
  {
    question: "모든 앱에서 사용할 수 있나요?",
    answer:
      "앱과 입력 필드가 운영체제의 접근성 기능을 제공해야 해요. 비밀번호 입력란이나 지원하지 않는 입력 필드는 교정하지 않아요. 사용 중인 앱에 따라 동작이 다를 수 있고, 설정에서 특정 앱을 교정 대상에서 제외할 수 있어요.",
  },
  {
    question: "설치 후 무엇을 설정해야 하나요?",
    answer:
      "앱을 열고 운영체제별 권한 안내를 따라주세요. macOS에서는 손쉬운 사용과 입력 모니터링 접근을 안내해요. 필요한 권한과 사전이 준비되면 교정이 활성화돼요. Windows 설치 파일은 Authenticode 서명이 없어 알 수 없는 게시자 또는 SmartScreen 안내가 표시될 수 있어요.",
  },
];

function FixKeyPage() {
  const [selectedPlatform, setSelectedPlatform] = useState(Platform.Mac);
  const selected = platforms.find(
    (platform) => platform.id === selectedPlatform,
  )!;
  return (
    <>
      <a className="skip-link" href="#main">
        본문으로 건너뛰기
      </a>
      <header className="site-header">
        <div className="nav-container">
          <a className="delino-logo" href="/" aria-label="Delino 홈">
            <span className="delino-symbol">d</span>delino
            <span className="logo-period">.</span>
          </a>
          <div className="nav-divider" />
          <a className="nav-product" href="/products/fixkey">
            FixKey
          </a>
          <nav aria-label="주 메뉴">
            <a href="#features">주요 기능</a>
            <a href="#privacy">개인정보 보호</a>
            <a href="#faq">자주 묻는 질문</a>
          </nav>
          <a className="nav-download" href="#download">
            다운로드
            <Icon name="arrow" />
          </a>
        </div>
      </header>
      <main id="main">
        <section
          className="hero section-container"
          aria-labelledby="hero-title"
        >
          <a
            className="release-pill"
            href={releaseUrl}
            target="_blank"
            rel="noreferrer"
          >
            <span className="pill-dot" /> 작은 실수를 덜어주는, FixKey
            <span className="pill-arrow">↗</span>
          </a>
          <h1 id="hero-title">
            한/영 전환은 잊고,
            <br />
            <span>생각만 이어가세요.</span>
          </h1>
          <p className="hero-description">
            분명 ‘안녕’을 썼는데 ‘dkssud’라면?
            <br />
            FixKey가 잘못된 키보드 배열을 알아보고, 의도한 단어로 바꿔드려요.
          </p>
          <div className="hero-actions">
            <a className="button button-primary" href="#download">
              <Icon name="download" /> FixKey 다운로드
            </a>
            <a className="button button-secondary" href="#demo">
              어떻게 동작하나요?<span className="play-icon">▶</span>
            </a>
          </div>
          <div className="hero-meta">
            <span>한국어 ↔ English</span>
            <i />
            <span>기기 안에서 안전하게</span>
            <i />
            <span>오픈 소스</span>
          </div>
          <Demo />
          <div className="platform-strip">
            <span>당신의 데스크톱에서, 조용히.</span>
            <div>
              <span>
                ⌘ <b>macOS</b>
              </span>
              <span>
                ⊞ <b>Windows</b>
              </span>
              <span>
                ⌁ <b>Linux</b>
              </span>
            </div>
          </div>
        </section>
        <section
          className="features section-container"
          id="features"
          aria-labelledby="features-title"
        >
          <div className="section-heading">
            <span className="eyebrow">MADE FOR YOUR FLOW</span>
            <h2 id="features-title">
              고치는 일은 줄이고,
              <br />
              하고 싶은 말에 집중하세요.
            </h2>
            <p>작은 방해가 사라지면, 생각은 더 멀리 이어지니까요.</p>
          </div>
          <div className="feature-grid">
            <article className="feature-card">
              <div className="feature-art art-correction">
                <span className="mini-word">dkssud</span>
                <span className="mini-arrow">→</span>
                <span className="mini-word result">
                  안녕
                  <Icon name="check" />
                </span>
                <span className="art-caption">
                  언어가 달라도, 의도는 그대로.
                </span>
              </div>
              <div className="feature-copy">
                <span className="feature-number">01 / SMART CORRECTION</span>
                <h3>잘못 누른 한/영, 알아서.</h3>
                <p>
                  완성된 단어를 사전과 비교해요.
                  <br />한 언어에서만 유효할 때 교정하고,
                  <br />
                  모호한 단어는 그대로 두어요.
                </p>
              </div>
            </article>
            <article className="feature-card">
              <div className="feature-art art-undo">
                <span className="undo-key">
                  ⌫<small>backspace</small>
                </span>
                <span className="undo-trail">↶</span>
                <span className="art-caption">돌아가는 것도, 손쉽게.</span>
              </div>
              <div className="feature-copy">
                <span className="feature-number">02 / YOU’RE IN CONTROL</span>
                <h3>원래대로, 간단하게.</h3>
                <p>
                  의도한 교정이 아니라면 되돌리세요.
                  <br />
                  필요할 때는 교정을 잠시 멈추거나,
                  <br />
                  특정 앱을 제외할 수도 있어요.
                </p>
              </div>
            </article>
            <article className="feature-card">
              <div className="feature-art art-local">
                <div className="local-orbit">
                  <Icon name="shield" />
                  <span className="orbit-dot one" />
                  <span className="orbit-dot two" />
                </div>
                <span className="local-tag">ON YOUR DEVICE</span>
                <span className="art-caption">내가 쓴 말은, 내 기기 안에.</span>
              </div>
              <div className="feature-copy">
                <span className="feature-number">03 / LOCAL BY DESIGN</span>
                <h3>내 기기에서 끝나는 교정.</h3>
                <p>
                  계정도, 클라우드 처리도 필요 없어요.
                  <br />
                  입력 내용과 교정 기록은
                  <br />
                  저장하거나 전송하지 않아요.
                </p>
              </div>
            </article>
          </div>
        </section>
        <section
          className="privacy section-container"
          id="privacy"
          aria-labelledby="privacy-title"
        >
          <div className="privacy-panel">
            <div className="privacy-visual" aria-hidden="true">
              <div className="privacy-ring outer" />
              <div className="privacy-ring inner" />
              <div className="privacy-lock">
                <Icon name="shield" />
              </div>
              <span className="privacy-chip">
                <span className="status-dot" /> 로컬에서 처리 중
              </span>
            </div>
            <div className="privacy-copy">
              <span className="eyebrow">YOUR WORDS STAY YOURS</span>
              <h2 id="privacy-title">
                당신의 말은,
                <br />
                당신만의 것이니까.
              </h2>
              <p>
                메모 속 아이디어부터 소중한 대화까지.
                <br />
                FixKey는 교정에 필요한 처리를 기기 안에서 끝내요.
              </p>
              <ul>
                <li>
                  <Icon name="check" />
                  입력 내용과 교정 기록 저장·전송 없음
                </li>
                <li>
                  <Icon name="check" />
                  비밀번호 입력란은 교정 대상에서 제외
                </li>
                <li>
                  <Icon name="check" />
                  네트워크는 업데이트 확인과 다운로드에만 사용
                </li>
              </ul>
            </div>
          </div>
        </section>
        <section
          className="faq section-container"
          id="faq"
          aria-labelledby="faq-title"
        >
          <div>
            <span className="eyebrow">A FEW GOOD QUESTIONS</span>
            <h2 id="faq-title">
              궁금한 점이
              <br />
              있으신가요?
            </h2>
            <a
              className="text-link"
              href="https://github.com/delinoio/apps/issues"
              target="_blank"
              rel="noreferrer"
            >
              GitHub에서 문의하기 <Icon name="arrow" />
            </a>
          </div>
          <div className="faq-list">
            {faqs.map((faq) => (
              <details key={faq.question}>
                <summary>
                  {faq.question}
                  <span className="faq-plus" aria-hidden="true" />
                </summary>
                <p>{faq.answer}</p>
              </details>
            ))}
          </div>
        </section>
        <section
          className="download section-container"
          id="download"
          aria-labelledby="download-title"
        >
          <div className="download-panel">
            <ProductMark />
            <span className="eyebrow">LESS FRICTION. MORE YOU.</span>
            <h2 id="download-title">
              이제, 생각의 흐름을
              <br />
              이어갈 시간.
            </h2>
            <p>키보드 위의 작은 도우미, FixKey와 함께하세요.</p>
            <div
              className="platform-picker"
              role="group"
              aria-label="다운로드 운영체제"
            >
              {platforms.map((platform) => (
                <button
                  key={platform.id}
                  aria-pressed={selectedPlatform === platform.id}
                  onClick={() => setSelectedPlatform(platform.id)}
                >
                  <span aria-hidden="true">{platform.symbol}</span>
                  {platform.name}
                </button>
              ))}
            </div>
            <a
              className="button button-primary"
              href={`${downloadBase}${selected.file}`}
            >
              <Icon name="download" />
              {selected.name}용 다운로드
            </a>
            <span className="download-detail">
              v0.1.7 <i /> {selected.detail}
            </span>
            <p className="platform-notice" aria-live="polite">
              {selectedPlatform === Platform.Windows
                ? "Windows에서 알 수 없는 게시자 또는 SmartScreen 안내가 표시될 수 있어요."
                : selectedPlatform === Platform.Linux
                  ? "X11 환경용이에요. Wayland 환경은 지원 대상에 포함되지 않아요."
                  : "Apple Silicon Mac용이에요. 설치 후 앱의 권한 안내를 확인해주세요."}
            </p>
            <a
              className="release-link"
              href={releaseUrl}
              target="_blank"
              rel="noreferrer"
            >
              릴리스 노트 및 모든 다운로드 ↗
            </a>
          </div>
        </section>
      </main>
      <footer className="site-footer section-container">
        <div>
          <a className="delino-logo" href="/">
            <span className="delino-symbol">d</span>delino
            <span className="logo-period">.</span>
          </a>
          <span className="footer-tagline">
            작은 도구로, 더 자연스러운 일상.
          </span>
        </div>
        <div className="footer-right">
          <a
            href="https://github.com/delinoio/apps"
            target="_blank"
            rel="noreferrer"
          >
            <Icon name="github" />
            GitHub
          </a>
          <span>© {new Date().getFullYear()} Delino</span>
        </div>
      </footer>
    </>
  );
}

function App() {
  const path = window.location.pathname.replace(/\/+$/, "") || "/";
  if (path === "/products/fixkey") {
    document.title = "FixKey — 한/영 전환은 잊고, 생각만 이어가세요 | Delino";
    document.documentElement.lang = "ko";
    return <FixKeyPage />;
  }
  return null;
}

createRoot(document.getElementById("root")!).render(<App />);
