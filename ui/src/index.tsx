import { render } from "solid-js/web";
import App from "./App";
import { inTauri, setApi } from "./ipc/api";
import "./theme/tokens.css";
import "./theme/controls.css";
import { reportUncaughtErrors } from "./stores/diagnostics";
import { initTheme } from "./theme/theme";

initTheme();
reportUncaughtErrors();

async function start() {
  // Outside Tauri (plain `npm run dev` in a browser) there is no backend: use the mock.
  if (!inTauri()) {
    const { createMockApi } = await import("./ipc/mock");
    const mock = createMockApi({
      latencyMs: 300,
      workspaces: {
        "/home/me/demo-api": {
          name: "Demo API",
          vars: { baseUrl: "https://api.example.com" },
          description: "A sample workspace served by the in-browser mock backend.",
          items: [
            {
              folder: "Users",
              children: [
                { request: "List users", url: "{{baseUrl}}/users?page=1" },
                { request: "Get user", url: "https://api.example.com/users/:id" },
                { request: "Create user", method: "POST", url: "https://api.example.com/users" },
              ],
            },
            {
              folder: "Orders",
              children: [{ request: "List orders", url: "https://api.example.com/orders" }],
            },
            { request: "Health", url: "https://api.example.com/health" },
            { error: "broken.yaml", message: "line 4: unresolved merge conflict marker <<<<<<<" },
          ],
        },
      },
      vars: {
        globals: { userAgentSuffix: "demo" },
        environments: {
          "/home/me/demo-api": [
            {
              schema: "ironcall/environment@1",
              id: "01MOCKENVDEV0000000000000",
              name: "Dev",
              vars: { baseUrl: "https://dev.api.example.com", apiKey: { secret: true } },
            },
            {
              schema: "ironcall/environment@1",
              id: "01MOCKENVPROD000000000000",
              name: "Prod",
              vars: { baseUrl: "https://api.example.com", apiKey: { secret: true } },
            },
          ],
        },
        secrets: { "environment:01MOCKENVDEV0000000000000/apiKey": "dev-key-123" },
      },
    });
    mock.disk.dialog.folder = "/home/me/new-api";
    setApi(mock);
  }
  const root = document.getElementById("root");
  if (!root) throw new Error("#root element missing from index.html");
  render(() => <App />, root);
}

void start();
