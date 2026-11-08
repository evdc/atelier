import "@fontsource/im-fell-english/latin-400.css";
import "@fontsource/im-fell-english/latin-400-italic.css";
import "@fontsource/source-serif-4/latin-400.css";
import "@fontsource/source-serif-4/latin-400-italic.css";
import "@fontsource/source-serif-4/latin-500.css";
import "@fontsource/fragment-mono/latin-400.css";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/app.css";

import { render } from "solid-js/web";
import { Conn, serverConfig } from "./lib/conn";
import { App } from "./ui/App";
import { Unconfigured } from "./ui/Unconfigured";

const root = document.getElementById("root")!;
const cfg = serverConfig();
render(() => (cfg ? <App conn={new Conn(cfg)} /> : <Unconfigured />), root);
