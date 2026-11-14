// SPDX-License-Identifier: Apache-2.0
import { define } from "rstack";

define.fmt({
  proseWrap: "preserve",
  embeddedLanguageFormatting: "off",
  ignorePatterns: [
    "**/dist/**",
    "**/node_modules/**",
    "**/vendor/**",
    ".agents/skills/**",
    "**/third-party/**",
    "**/testdata/**",
    "**/fixtures/**",
    "protos/gen/**",
    "packages/*/src/gen/**",
    "**/*.generated.*",
    "**/pnpm-lock.yaml",
    "**/package-lock.json",
    "**/npm-shrinkwrap.json",
    "**/yarn.lock",
  ],
});
