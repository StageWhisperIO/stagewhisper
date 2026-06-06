import { useEffect, useState } from "react";
import { useTheme } from "@/hooks/useTheme";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { SidebarItem } from "./primitives";
import { GeneralIcon, SunIcon, MoonIcon } from "./icons";
import { LocalPipelineSection } from "./local-pipeline";
import { ConnectionSection } from "./connection";
import { LibrarySection } from "./library";
import { AboutSection } from "./about";

type SidebarSection = "connection" | "model" | "library" | "about";

export function SettingsWindow() {
  const { theme, toggle: toggleTheme } = useTheme();

  const [activeSection, setActiveSection] = useState<SidebarSection>("connection");
  const [librarySession, setLibrarySession] = useState<string | null>(null);

  useEffect(() => {
    const unlistenFinalized = listen<{ session_id: string }>("session-finalized", (event) => {
      setLibrarySession(event.payload.session_id);
      setActiveSection("library");
      void invoke("open_settings_window");
    });

    const unlistenNavigate = listen<{ section: SidebarSection }>("settings-navigate", (event) => {
      const section = event.payload?.section;
      if (
        section === "connection" ||
        section === "model" ||
        section === "library" ||
        section === "about"
      ) {
        setActiveSection(section);
      }
    });

    return () => {
      unlistenFinalized.then((fn) => fn());
      unlistenNavigate.then((fn) => fn());
    };
  }, []);

  const handleQuit = () => {
    void invoke("quit_app");
  };

  return (
    <main className="flex h-full w-full bg-page text-body">
      <aside className="flex w-56 shrink-0 flex-col border-r border-rule bg-panel">
        <div className="h-12 shrink-0" data-tauri-drag-region />
        <div className="flex items-center justify-between px-5 pb-4">
          <h1 className="text-lg font-semibold text-heading">Settings</h1>
        </div>

        <nav className="flex-1 space-y-0.5 px-3">
          <SidebarItem
            icon={<ConnectionIcon />}
            label="Connection"
            active={activeSection === "connection"}
            onClick={() => setActiveSection("connection")}
          />
          <SidebarItem
            icon={<ModelIcon />}
            label="Model"
            active={activeSection === "model"}
            onClick={() => setActiveSection("model")}
          />
          <SidebarItem
            icon={<LibraryIcon />}
            label="Library"
            active={activeSection === "library"}
            onClick={() => {
              setLibrarySession(null);
              setActiveSection("library");
            }}
          />
          <SidebarItem
            icon={<GeneralIcon />}
            label="About"
            active={activeSection === "about"}
            onClick={() => setActiveSection("about")}
          />
        </nav>

        <footer className="border-t border-rule px-4 py-4 space-y-2">
          <div className="flex items-center justify-between mb-3">
            <span className="text-xs font-medium text-muted">Theme</span>
            <button
              onClick={toggleTheme}
              className="flex items-center gap-1.5 rounded-lg border border-rule-strong px-2.5 py-1.5 text-xs font-medium text-body transition hover:border-accent hover:text-accent"
              title={theme === "dark" ? "Switch to light" : "Switch to dark"}
            >
              {theme === "dark" ? <SunIcon /> : <MoonIcon />}
              {theme === "dark" ? "Light" : "Dark"}
            </button>
          </div>
          <button
            onClick={handleQuit}
            className="w-full rounded-lg border border-rule-strong px-3 py-2 text-xs font-medium text-body transition hover:bg-sidebar-active"
          >
            Quit
          </button>
        </footer>
      </aside>

      <div className="flex flex-1 flex-col overflow-hidden bg-page">
        <div className="h-8 shrink-0" data-tauri-drag-region />
        <div className="min-h-0 flex-1">
          {activeSection === "connection" && <ConnectionSection />}
          {activeSection === "model" && <LocalPipelineSection />}
          {activeSection === "library" && (
            <LibrarySection key={librarySession ?? "list"} initialSessionId={librarySession} />
          )}
          {activeSection === "about" && <AboutSection />}
        </div>
      </div>
    </main>
  );
}

function ModelIcon() {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <rect x="4" y="4" width="16" height="6" rx="1" />
      <rect x="4" y="14" width="16" height="6" rx="1" />
      <line x1="8" y1="7" x2="8" y2="7" />
      <line x1="8" y1="17" x2="8" y2="17" />
    </svg>
  );
}

function LibraryIcon() {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20" />
      <path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z" />
    </svg>
  );
}

function ConnectionIcon() {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" />
      <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
    </svg>
  );
}
