import android.media.MediaCodecInfo;
import android.media.MediaCodecList;
import org.json.JSONArray;
import org.json.JSONObject;

public class CodecInventory {
    public static void main(String[] args) throws Exception {
        JSONArray codecs = new JSONArray();
        for (MediaCodecInfo info : new MediaCodecList(MediaCodecList.ALL_CODECS).getCodecInfos()) {
            if (info.isEncoder()) continue;
            JSONArray types = new JSONArray();
            for (String type : info.getSupportedTypes()) {
                if (type.startsWith("video/")) types.put(type);
            }
            if (types.length() == 0) continue;
            codecs.put(new JSONObject().put("name", info.getName())
                .put("canonical_name", info.getCanonicalName()).put("types", types)
                .put("hardware_accelerated", info.isHardwareAccelerated())
                .put("software_only", info.isSoftwareOnly()).put("vendor", info.isVendor()));
        }
        System.out.println(new JSONObject().put("video_decoders", codecs));
    }
}
