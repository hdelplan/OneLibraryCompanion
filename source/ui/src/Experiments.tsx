import { TestScreen } from "./TestScreen";
import { CueWindow } from "./CueWindow";
import type { OfflinePreviewProps } from "./OfflinePreview";
export default function Experiments(
  props: OfflinePreviewProps & {
    page: string;
    liveEnabled: boolean;
    selectLive: () => void;
    zeroSmoothing: () => void;
  },
) {
  return (
    <TestScreen
      {...props}
      active={props.page === "test"}
      cueWindow={
        <CueWindow
          test={props.page === "test"}
          status={props.page === "status"}
          onStatus={props.selectLive}
        />
      }
    />
  );
}
