import DefaultTheme from "vitepress/theme";
import Layout from "./Layout.vue";
import "@fontsource/manrope/latin-400.css";
import "@fontsource/manrope/latin-500.css";
import "@fontsource/manrope/latin-600.css";
import "@fontsource/manrope/latin-700.css";
import "@fontsource/manrope/latin-800.css";
import "@fontsource/dm-mono/latin-400.css";
import "@fontsource/dm-mono/latin-500.css";
import "./style.css";

export default {
  extends: DefaultTheme,
  Layout,
};
