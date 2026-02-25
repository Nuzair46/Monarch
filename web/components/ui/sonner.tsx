import { Toaster as Sonner, type ToasterProps } from "sonner";

const Toaster = ({ ...props }: ToasterProps) => (
  <Sonner
    theme="dark"
    toastOptions={{
      classNames: {
        toast: "bg-background text-foreground border-border",
      },
    }}
    {...props}
  />
);

export { Toaster };
