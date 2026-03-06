import { StrictMode } from "react";
import ReactDOM from "react-dom/client";
import { Toaster } from "@/components/ui/sonner";
import App from "./App";
import "./styles.css";

const rootElement = document.getElementById("root");
if (!rootElement) {
  throw new Error("Root element not found");
}

const htmlElement = document.documentElement;
htmlElement.classList.add("dark");
htmlElement.style.colorScheme = "dark";

ReactDOM.createRoot(rootElement).render(
  <StrictMode>
    <App />
    <Toaster position="bottom-right" />
  </StrictMode>,
);
