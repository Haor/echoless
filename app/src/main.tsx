import "./devBrowserShim";
import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { LangProvider } from "./i18n";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <LangProvider>
      <ErrorBoundary label="root">
        <App />
      </ErrorBoundary>
    </LangProvider>
  </React.StrictMode>,
);
