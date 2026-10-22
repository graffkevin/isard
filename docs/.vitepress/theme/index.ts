import DefaultTheme from 'vitepress/theme';
import { h } from 'vue';
import ArchitectureDiagram from './ArchitectureDiagram.vue';
import HomeGlobe from './HomeGlobe.vue';
import LiveExample from './LiveExample.vue';
import './style.css';

export default {
  extends: DefaultTheme,
  // The live globe between the home page's hero and its features
  Layout: () => h(DefaultTheme.Layout, null, { 'home-hero-after': () => h(HomeGlobe) }),
  enhanceApp({ app }) {
    app.component('ArchitectureDiagram', ArchitectureDiagram);
    app.component('LiveExample', LiveExample);
  },
};
