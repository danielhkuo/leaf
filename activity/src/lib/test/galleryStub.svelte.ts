// Test support (views/Viewer.test.ts): a reactive stand-in for the gallery
// store's state, so effects that read `gallery.epoch` rerun when it changes.
export const galleryStub = $state({ epoch: 0 });
