// SPDX-License-Identifier: Apache-2.0

const sections = [
  {
    id: "account",
    title: "Account",
    description: "Manage your profile and preferences.",
  },
  {
    id: "billing",
    title: "Billing",
    description: "Manage payment methods, balances and invoices.",
  },
] as const;

export function App() {
  return (
    <main className="page">
      <header className="page-header">
        <h1>Delino Accounts</h1>
        <p>Manage your Delino account and billing information.</p>
      </header>
      <ul className="sections" role="list">
        {sections.map((section) => (
          <li key={section.id} className="section">
            <h2>{section.title}</h2>
            <p>{section.description}</p>
            <span className="badge">Coming soon</span>
          </li>
        ))}
      </ul>
    </main>
  );
}
