import { Github } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Card,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { TabsList, TabsTrigger } from "@/components/ui/tabs";
import { openExternalUrl } from "@/tauri";
import { REPO_URL, VIEW_OPTIONS } from "@/app/ui";

export function AppHeader() {
  return (
    <Card className="overflow-hidden">
      <CardHeader className="gap-4 lg:flex-row lg:items-start lg:justify-between">
        <div className="space-y-3">
          <div className="space-y-2">
            <CardTitle className="tracking-widest text-primary">
              MONARCH
            </CardTitle>
            <CardDescription className="max-w-3xl text-sm leading-relaxed sm:text-base">
              Detach, restore, and switch monitor layouts without touching cables.
            </CardDescription>
          </div>
        </div>

        <div className="flex w-full flex-col gap-2 lg:w-auto lg:items-end">
          <TabsList className="h-auto w-full justify-start p-1 lg:w-auto">
            {VIEW_OPTIONS.map(({ id, label }) => (
              <TabsTrigger key={id} value={id} className="min-w-[96px]">
                {label}
              </TabsTrigger>
            ))}
          </TabsList>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="justify-start lg:justify-end"
            aria-label="Open Monarch GitHub repository"
            onClick={() => {
              void openExternalUrl(REPO_URL);
            }}
          >
            <Github />
            GitHub
          </Button>
        </div>
      </CardHeader>
    </Card>
  );
}
