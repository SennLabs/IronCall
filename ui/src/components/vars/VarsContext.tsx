import { type Accessor, createContext, useContext } from "solid-js";
import type { VariableContext } from "../../ipc/api";

/**
 * The active tab's variables, for every variable-aware input below it (URL bar, table
 * cells, body editor). Without a provider, inputs behave like plain inputs.
 */
const VarsContext = createContext<Accessor<VariableContext | undefined>>();

export const VarsProvider = VarsContext.Provider;

export function useVars(): Accessor<VariableContext | undefined> | undefined {
  return useContext(VarsContext);
}
