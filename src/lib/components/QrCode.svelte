<script lang="ts">
  let { svg }: { svg: string } = $props();
</script>

<div class="qr-container" aria-label="QR code for device linking">
  {@html svg}
</div>

<style>
  .qr-container {
    display: flex;
    align-items: center;
    justify-content: center;
    pointer-events: none;
    user-select: none;
    width: 280px;
    height: 280px;
    border-radius: 16px;
    overflow: hidden;
    background: #1a1a2e;
  }

  .qr-container :global(svg) {
    width: 100%;
    height: 100%;
  }
</style>
