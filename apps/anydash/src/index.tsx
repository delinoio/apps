// SPDX-License-Identifier: Apache-2.0
import "./polyfills";
import { root } from "@lynx-js/react";
import "@packages/lynx-ui/styles.css";
import "./style.css";
import { App } from "./App";

const props = (lynx.__globalProps ?? {}) as { initialPath?: string };
root.render(<App initialPath={props.initialPath || "/"} />);
