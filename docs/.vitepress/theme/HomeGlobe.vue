<script setup lang="ts">
import LiveExample from './LiveExample.vue';
</script>

<!-- The home page's live globe, wide, right under the hero: the engine shown before it is described -->
<template>
  <div class="home-globe">
    <LiveExample slug="globe" height="min(680px, 72vh)" />
  </div>
</template>

<style scoped>
.home-globe {
  max-width: 1280px;
  margin: 0 auto 32px;
  padding: 0 24px;
}
</style>
