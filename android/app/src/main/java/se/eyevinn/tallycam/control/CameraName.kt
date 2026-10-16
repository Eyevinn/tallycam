package se.eyevinn.tallycam.control

/**
 * The camera's display name, sent as `hello.device.name` (docs/PROTOCOL.md):
 * set by the operator (`camera_name`) or in the app, by default the model
 * and the start of the device id ("Pixel 8 · 55dd"), as on iOS.
 */
object CameraName {
    const val MAX_LENGTH = 64

    /** Trimmed and cut to [MAX_LENGTH]; null when empty (the default). */
    fun normalized(name: String?): String? =
        name?.trim()?.takeIf { it.isNotEmpty() }?.take(MAX_LENGTH)

    /** `custom`, or "<model> · <first 4 of the device id>". */
    fun display(custom: String?, model: String, deviceId: String?): String {
        normalized(custom)?.let { return it }
        if (deviceId.isNullOrEmpty()) return model
        return "$model · ${deviceId.take(4).lowercase()}"
    }

    /**
     * A readable model: `Build.MODEL`, with the manufacturer in front unless
     * the model already names it ("Pixel 8", "Samsung SM-S921B").
     */
    fun model(manufacturer: String, model: String): String {
        val maker = manufacturer.replaceFirstChar { it.uppercase() }
        return when {
            model.startsWith(manufacturer, ignoreCase = true) -> model
            model.startsWith("Pixel") -> model
            else -> "$maker $model"
        }
    }
}
