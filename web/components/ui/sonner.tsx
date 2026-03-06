import { Toaster as Sonner, type ToasterProps } from "sonner";

const toasterClassNames: NonNullable<ToasterProps["toastOptions"]>["classNames"] = {
  toast: "bg-background text-foreground border-border",
};

const Toaster = (props: ToasterProps) => (
  <Sonner
    theme="dark"
    toastOptions={{
      classNames: toasterClassNames,
    }}
    {...props}
  />
);

export { Toaster };
