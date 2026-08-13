import { MenuPopover } from "./features/menu/MenuPopover";
import { SettingsWindow } from "./features/settings/SettingsWindow";

export type AppView = "menu" | "settings";

interface AppProps {
  view: AppView;
}

export function App({ view }: AppProps) {
  return view === "settings" ? <SettingsWindow /> : <MenuPopover />;
}
