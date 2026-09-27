import { Github, Layers, Monitor, Settings2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { TabsList, TabsTrigger } from "@/components/ui/tabs";
import { openExternalUrl } from "@/tauri";
import { REPO_URL, VIEW_OPTIONS } from "@/app/ui";
import packageJson from "../../../package.json";

const icons = { main: Monitor, profiles: Layers, settings: Settings2 };

export function AppHeader() {
  return (
    <header className="border-b bg-background">
      <div className="mx-auto flex min-h-16 max-w-[1440px] flex-wrap items-center justify-between gap-x-6 gap-y-3 px-4 py-3 sm:px-5">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <h1 className="text-lg font-semibold">Monarch</h1>
            <span className="shortcut">v{packageJson.version}</span>
          </div>
          <p className="text-xs text-muted-foreground">
            Display &amp; audio profiles
          </p>
        </div>
        <div className="order-3 w-full sm:order-none sm:w-auto">
          <TabsList aria-label="Main navigation" className="w-full sm:w-auto">
            {VIEW_OPTIONS.map(({ id, label }) => {
              const Icon = icons[id];
              return (
                <TabsTrigger
                  key={id}
                  value={id}
                  className="flex-1 sm:flex-none"
                >
                  <Icon className="size-4" aria-hidden="true" />
                  {label}
                </TabsTrigger>
              );
            })}
          </TabsList>
        </div>
        <Button
          type="button"
          variant="outline"
          size="icon"
          title="Open Monarch on GitHub"
          aria-label="Open Monarch GitHub repository"
          onClick={() => {
            void openExternalUrl(REPO_URL);
          }}
        >
          <Github aria-hidden="true" />
        </Button>
      </div>
    </header>
  );
}
