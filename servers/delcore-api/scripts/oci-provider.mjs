// SPDX-License-Identifier: FSL-1.1-ALv2
// Disposable OCI fixture. Never use this provider outside isolated tests.
import { createServer } from "node:http";

const transactions = [];
createServer(async (request, response) => {
  response.setHeader("Content-Type", "application/json");
  const path = new URL(request.url, "http://127.0.0.1:8099").pathname;
  const send = (value) => response.end(JSON.stringify(value));
  if (path === "/.well-known/openid-configuration") {
    send({
      issuer: "http://127.0.0.1:8099",
      jwks_uri: "http://127.0.0.1:8099/keys",
      authorization_endpoint: "http://127.0.0.1:8099/auth",
      token_endpoint: "http://127.0.0.1:8099/token",
      id_token_signing_alg_values_supported: ["RS256"],
    });
  } else if (path === "/keys") send({ keys: [] });
  else if (path === "/transactions" && request.method === "GET")
    send({ data: transactions });
  else if (path === "/transactions" && request.method === "POST") {
    let body;
    try {
      const chunks = [];
      for await (const chunk of request) chunks.push(chunk);
      body = JSON.parse(Buffer.concat(chunks));
    } catch {
      response.writeHead(400);
      response.end();
      return;
    }
    const transaction = {
      id: "txn_crash_fixture",
      status: "completed",
      currency_code: "USD",
      customer_id: "ctm_crash_fixture",
      custom_data: body.custom_data,
      items: body.items,
      details: { totals: { subtotal: "1000" } },
    };
    transactions.push(transaction);
    // Persist provider acceptance before replying so the test can kill the API
    // in the ambiguous payment window and require read-only reconciliation.
    setTimeout(() => send({ data: transaction }), 30000);
  } else {
    response.writeHead(404);
    response.end();
  }
}).listen(8099, "127.0.0.1");
