/// <reference types="vite/client" />

// @fontsource packages ship CSS with no type declarations; the bare side-effect
// imports need ambient module declarations (TS 6 errors on them otherwise).
declare module "@fontsource/geist-sans";
declare module "@fontsource/geist-mono";
