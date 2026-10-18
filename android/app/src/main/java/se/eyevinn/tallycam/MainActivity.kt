package se.eyevinn.tallycam

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Bundle
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import se.eyevinn.tallycam.control.ProvisioningLink
import se.eyevinn.tallycam.ui.TallyCamScreen

class MainActivity : ComponentActivity() {
    private var model: AppModel? = null
    /** Recreated (rotation, process restore): the launch link was applied already. */
    private var restored = false
    private var permitted by mutableStateOf(false)
    /** A link to another control hub, waiting for the operator's "Join" (D1). */
    private var pendingLink by mutableStateOf<ProvisioningLink?>(null)

    private val permissions = registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
        permitted = it[Manifest.permission.CAMERA] == true
        if (permitted) start()
    }

    /** The in-app QR scanner, for phones whose camera app does not open tallycam:// links. */
    private val scanner = registerForActivityResult(ScanContract()) { result ->
        val text = result.contents ?: return@registerForActivityResult
        val link = runCatching { android.net.Uri.parse(text) }.getOrNull()?.let(ProvisioningLink::parse)
        if (link != null) offer(link) else model?.showMessage("Not a TallyCam code")
    }

    private fun scan() {
        scanner.launch(ScanOptions().setDesiredBarcodeFormats(ScanOptions.QR_CODE).setBeepEnabled(false)
                           .setPrompt("Scan the QR code in the control hub").setOrientationLocked(false))
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        restored = savedInstanceState != null
        // A camera on a tripod must not lock itself.
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        WindowCompat.setDecorFitsSystemWindows(window, false)
        WindowInsetsControllerCompat(window, window.decorView).apply {
            hide(WindowInsetsCompat.Type.systemBars())
            systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        }
        val needed = arrayOf(Manifest.permission.CAMERA, Manifest.permission.RECORD_AUDIO)
        permitted = needed.all { checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED }
        if (permitted) start() else permissions.launch(needed)
        show()
    }

    private fun show() = setContent {
        TallyCamScreen(model, permitted, ::scan, pendingLink,
                       onJoin = { link -> pendingLink = null; model?.provision(link) },
                       onDecline = { pendingLink = null })
    }

    private fun start() {
        if (model != null) return
        model = TallyCam.model(this)
        if (!restored) handle(intent)
        show()
        // Android 13+: the streaming notification needs this; streaming does not.
        if (android.os.Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
        }
    }

    override fun onStart() {
        super.onStart()
        model?.uiVisible = true
    }

    override fun onStop() {
        super.onStop()
        model?.uiVisible = false
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        // Kept for start() when the camera permission is still being asked.
        setIntent(intent)
        handle(intent)
    }

    /** tallycam://connect?... from the system camera's QR scanner. */
    private fun handle(intent: Intent?) {
        val uri = intent?.data ?: return
        val link = ProvisioningLink.parse(uri)
        if (link != null) offer(link) else model?.showMessage("Not a valid connect code")
    }

    /**
     * A link for the control hub already in use applies at once; one for
     * another hub (or the first) waits for the operator to join it, so a
     * stray QR code never takes the camera over (D1).
     */
    private fun offer(link: ProvisioningLink) {
        val model = model ?: return
        if (server(link.controlUrl) == server(model.settings.controlUrl)) {
            model.provision(link)
        } else {
            pendingLink = link
        }
    }

    /** "host:port" of a URL, for "the same hub"; null when it has no host (none set). */
    private fun server(url: String): String? =
        runCatching { android.net.Uri.parse(url.trim()) }.getOrNull()?.takeIf { !it.host.isNullOrEmpty() }
            ?.let { "${it.host}:${it.port}" }

    // The model lives as long as the process (TallyCam.model): a stream
    // goes on while the activity is gone, in StreamService's foreground.
}
