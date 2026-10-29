package app.polarbear

import kotlin.math.hypot

fun main() {
    val bounds = CapsulePosition.bounds(1000f,700f,120f,48f,24f,30f,20f,25f,8f)
    check(bounds.clamp(-50f,-50f) == 32f to 38f)
    check(bounds.clamp(2000f,2000f) == 852f to 619f)
    check(CapsulePosition.remembered(bounds,1000f,700f,120f,0.5f,-1f) == 440f to 38f)
    val small = CapsulePosition.bounds(300f,200f,180f,48f,0f,40f,0f,240f,8f)
    check(small.clamp(500f,500f) == 112f to 48f)
    val tinyBounds = CapsulePosition.bounds(80f,30f,120f,48f,30f,40f,30f,40f,8f)
    check(tinyBounds.clamp(-100f,300f) == 0f to 0f)
    check(CapsulePosition.remembered(bounds,1000f,700f,120f,Float.NaN,Float.POSITIVE_INFINITY) == 440f to 38f)
    val rotated = CapsulePosition.bounds(700f,1000f,180f,48f,0f,30f,0f,25f,8f)
    check(CapsulePosition.remembered(rotated,700f,1000f,180f,0.98f,0.98f) == 512f to 919f)
    val area = UtilityMenuGeometry.Area(20f,30f,980f,670f)
    for (count in 4..6) for (font in listOf(1f,1.5f,2f)) {
        val menu = UtilityMenuGeometry.menu(area,UtilityMenuGeometry.Point(970f,70f),count,1f,font)
        check(!menu.compact)
        menu.offsets.forEach { p ->
            check(menu.center.x+p.x-menu.tileWidth/2 >= area.left)
            check(menu.center.x+p.x+menu.tileWidth/2 <= area.right)
            check(menu.center.y+p.y-menu.tileHeight/2 >= area.top)
            check(menu.center.y+p.y+menu.tileHeight/2 <= area.bottom)
        }
        for (i in menu.offsets.indices) for (j in 0 until i) {
            val a=menu.offsets[i]; val b=menu.offsets[j]
            check(hypot(a.x-b.x,a.y-b.y) >= 90f)
        }
    }
    for (narrow in listOf(UtilityMenuGeometry.Area(0f,20f,300f,200f),UtilityMenuGeometry.Area(0f,0f,180f,500f))) {
        check(UtilityMenuGeometry.menu(narrow,UtilityMenuGeometry.Point(20f,30f),6,1f,1.5f).compact)
    }
    val point=UtilityMenuGeometry.Point(400f,200f)
    val slow=UtilityMenuGeometry.released(area,point,UtilityMenuGeometry.Point(0f,0f),1f)
    val flick=UtilityMenuGeometry.released(area,point,UtilityMenuGeometry.Point(1600f,900f),1f)
    check(!slow.second && flick.second)
    check(flick.first.y > slow.first.y)
    check(flick.first.x == area.right-20)
    val clipped=UtilityMenuGeometry.released(area,point,UtilityMenuGeometry.Point(-90000f,90000f),1f)
    check(clipped.first.y <= area.bottom-24 && clipped.first.y >= area.top+24)
    val tiny=UtilityMenuGeometry.dock(UtilityMenuGeometry.Area(0f,0f,80f,30f),true,100f,1f)
    check(tiny.y in 0f..30f)
    println("Utility docking momentum, menu spacing, insets, text scaling and compact layout checks passed")
}
