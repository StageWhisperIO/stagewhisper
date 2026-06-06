interface Suggestion {
  id: number;
  type: "rebuttal" | "action" | "info";
  title: string;
  content: string;
}
