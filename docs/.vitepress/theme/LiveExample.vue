<script setup lang="ts">
import { withBase } from 'vitepress';

/** A live example, its page in a frame: the map runs in its own document, as an application would */
defineProps<{ slug: string; height?: string }>();
</script>

<template>
  <div class="example">
    <iframe :src="withBase(`/live/${slug}/`)" :style="{ height: height ?? '480px' }" loading="lazy" allow="fullscreen" />
    <a class="open" :href="withBase(`/live/${slug}/`)" target="_blank" rel="noopener">Open full page ↗</a>
  </div>
</template>

<style scoped>
.example {
  position: relative;
  margin: 16px 0;
  border: 1px solid var(--vp-c-divider);
  border-radius: 12px;
  overflow: hidden;
}
iframe {
  display: block;
  width: 100%;
  border: 0;
  background: #f2f2f0;
}
.open {
  position: absolute;
  right: 10px;
  bottom: 10px;
  padding: 2px 10px;
  border-radius: 999px;
  background: var(--vp-c-bg);
  font-size: 13px;
  text-decoration: none;
  box-shadow: 0 1px 6px rgb(0 0 0 / 0.15);
}
</style>
