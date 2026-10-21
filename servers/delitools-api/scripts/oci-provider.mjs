// SPDX-License-Identifier: FSL-1.1-ALv2
// Disposable OCI fixture: a minimal OIDC discovery document with an empty key set.
// Never use this provider outside isolated tests.
import { createServer } from "node:http";

createServer((request, response) => {
  response.setHeader("Content-Type", "application/json");
  const path = new URL(request.url, "http://127.0.0.1:8099").pathname;
  if (path === "/.well-known/openid-configuration") {
    response.end(
      JSON.stringify({
        issuer: "http://127.0.0.1:8099",
        jwks_uri: "http://127.0.0.1:8099/keys",
        id_token_signing_alg_values_supported: ["RS256"],
      }),
    );
  } else if (path === "/keys") {
    response.end(JSON.stringify({ keys: [] }));
  } else {
    response.writeHead(404);
    response.end("{}");
  }
}).listen(8099, "127.0.0.1");
